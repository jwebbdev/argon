use crossbeam_channel::Receiver;
use log::trace;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, FileIdMap};
use std::{
	collections::HashMap,
	io::{self, Result},
	path::{Path, PathBuf},
	sync::{mpsc, Arc, Mutex, RwLock},
	thread::Builder,
	time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use notify::event::DataChange;

#[cfg(not(target_os = "windows"))]
use notify::event::ModifyKind;

#[cfg(target_os = "linux")]
use notify::event::{AccessKind, AccessMode, RenameMode};

use super::VfsEvent;
use crate::{constants::SYNCBACK_DEBOUNCE_TIME, lock};

#[cfg(target_os = "linux")]
const DEBOUNCE_TIME: Duration = Duration::from_micros(500);

macro_rules! event_path {
	($event:expr) => {
		$event.paths.first().unwrap().to_owned()
	};
}

#[cfg(target_os = "linux")]
struct DebounceContext {
	time: Instant,
	path: PathBuf,
}

/// Paths that Argon itself has just written, mapped to the time of
/// the write. The file system events they trigger are only echoes of
/// changes the client already knows about, so they must not be synced
/// back. Unlike a blanket time window this drops the exact paths we
/// wrote and nothing else, so user changes are never lost
#[derive(Default)]
struct EchoMap {
	inner: HashMap<PathBuf, Instant>,
}

impl EchoMap {
	/// Remembers `path` as written by Argon so the event it causes
	/// can later be recognized as an echo
	fn record(&mut self, path: &Path) {
		self.sweep();
		self.inner.insert(path.to_owned(), Instant::now());
	}

	/// Returns `true` if `path` was written by Argon recently enough
	/// for the event to be an echo, consuming the entry that matched
	fn consume(&mut self, path: &Path) -> bool {
		self.sweep();
		self.inner.remove(path).is_some()
	}

	/// Forgets entries that are too old to be echoes, so the map stays
	/// bounded even when the events we expected never arrive
	fn sweep(&mut self) {
		self.inner.retain(|_, time| time.elapsed() < SYNCBACK_DEBOUNCE_TIME);
	}
}

pub struct VfsDebouncer {
	inner: Debouncer<RecommendedWatcher, FileIdMap>,
	is_paused: Arc<RwLock<bool>>,
	echoes: Arc<Mutex<EchoMap>>,
	receiver: Receiver<VfsEvent>,
}

impl VfsDebouncer {
	pub fn new() -> Self {
		let (inner_sender, inner_receiver) = mpsc::channel();
		let (sender, receiver) = crossbeam_channel::unbounded();

		let debouncer = new_debouncer(Duration::from_millis(100), None, inner_sender, false).unwrap();

		let is_paused = Arc::new(RwLock::new(false));
		let local_is_paused = is_paused.clone();

		let echoes = Arc::new(Mutex::new(EchoMap::default()));
		let local_echoes = echoes.clone();

		Builder::new()
			.name("debouncer".into())
			.spawn(move || {
				#[cfg(target_os = "linux")]
				let mut context = DebounceContext {
					time: Instant::now(),
					path: PathBuf::new(),
				};

				for events in inner_receiver {
					if *local_is_paused.read().unwrap() {
						continue;
					}

					for event in events.unwrap() {
						trace!("Debouncing event, paths: {:?}, kind: {:?}", event.paths, event.kind);

						#[cfg(not(target_os = "linux"))]
						let event = debounce(&event);

						#[cfg(target_os = "linux")]
						let event = debounce(&event, &mut context);

						if let Some(event) = event {
							if lock!(local_echoes).consume(event.path()) {
								trace!("Skipping event, {:?} was written by us", event.path());
								continue;
							}

							sender.send(event).unwrap();
						}
					}
				}
			})
			.unwrap();

		Self {
			inner: debouncer,
			is_paused,
			echoes,
			receiver,
		}
	}

	pub fn watch(&mut self, path: &Path, recursive: bool) -> Result<()> {
		let recursive = if recursive {
			RecursiveMode::Recursive
		} else {
			RecursiveMode::NonRecursive
		};

		self.inner.watcher().watch(path, recursive).map_err(map_error)?;
		self.inner.cache().add_root(path, recursive);

		Ok(())
	}

	pub fn unwatch(&mut self, path: &Path) -> Result<()> {
		self.inner.watcher().unwatch(path).map_err(map_error)?;
		self.inner.cache().remove_root(path);

		Ok(())
	}

	/// Marks `path` as written by Argon, so that the event it triggers
	/// gets skipped instead of being synced back to the client
	pub fn record(&self, path: &Path) {
		lock!(self.echoes).record(path);
	}

	pub fn pause(&mut self) {
		*self.is_paused.write().unwrap() = true;
	}

	pub fn resume(&mut self) {
		*self.is_paused.write().unwrap() = false;
	}

	pub fn receiver(&self) -> Receiver<VfsEvent> {
		self.receiver.clone()
	}
}

fn map_error(err: notify::Error) -> io::Error {
	match err.kind {
		notify::ErrorKind::Io(err) => err,
		notify::ErrorKind::PathNotFound => io::Error::new(io::ErrorKind::NotFound, err),
		notify::ErrorKind::WatchNotFound => io::Error::new(io::ErrorKind::NotFound, err),
		_ => io::Error::other(err),
	}
}

#[cfg(target_os = "macos")]
fn debounce(event: &DebouncedEvent) -> Option<VfsEvent> {
	match event.kind {
		EventKind::Create(_) => {
			let path = event_path!(event);

			if path.exists() {
				Some(VfsEvent::Create(path))
			} else {
				None
			}
		}
		EventKind::Remove(_) => Some(VfsEvent::Delete(event_path!(event))),
		EventKind::Modify(kind) => match kind {
			ModifyKind::Name(_) => {
				let path = event_path!(event);

				if path.exists() {
					Some(VfsEvent::Create(path))
				} else {
					Some(VfsEvent::Delete(path))
				}
			}
			ModifyKind::Data(kind) => {
				if kind == DataChange::Content {
					Some(VfsEvent::Write(event_path!(event)))
				} else {
					None
				}
			}
			_ => None,
		},
		_ => None,
	}
}

#[cfg(target_os = "linux")]
fn debounce(event: &DebouncedEvent, context: &mut DebounceContext) -> Option<VfsEvent> {
	match event.kind {
		EventKind::Create(_) => {
			let path = event_path!(event);

			context.time = event.time;
			context.path.clone_from(&path);

			Some(VfsEvent::Create(path))
		}
		EventKind::Remove(_) => Some(VfsEvent::Delete(event_path!(event))),
		EventKind::Modify(ModifyKind::Name(mode)) => match mode {
			RenameMode::From => Some(VfsEvent::Delete(event_path!(event))),
			RenameMode::To => Some(VfsEvent::Create(event_path!(event))),
			_ => None,
		},
		EventKind::Access(kind) => {
			if kind == AccessKind::Close(AccessMode::Write) {
				let duration = event.time.duration_since(context.time);
				let path = event_path!(event);

				if duration < DEBOUNCE_TIME && path == context.path {
					return None;
				}

				Some(VfsEvent::Write(path))
			} else {
				None
			}
		}
		_ => None,
	}
}

#[cfg(target_os = "windows")]
fn debounce(event: &DebouncedEvent) -> Option<VfsEvent> {
	match event.kind {
		EventKind::Create(_) => Some(VfsEvent::Create(event_path!(event))),
		EventKind::Remove(_) => Some(VfsEvent::Delete(event_path!(event))),
		EventKind::Modify(_) => Some(VfsEvent::Write(event_path!(event))),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::EchoMap;
	use crate::constants::SYNCBACK_DEBOUNCE_TIME;
	use std::{path::PathBuf, thread, time::Duration};

	fn expire() {
		thread::sleep(SYNCBACK_DEBOUNCE_TIME + Duration::from_millis(50));
	}

	#[test]
	fn echo_within_ttl_is_dropped() {
		let mut echoes = EchoMap::default();
		let path = PathBuf::from("src/init.luau");

		echoes.record(&path);

		assert!(echoes.consume(&path));
	}

	#[test]
	fn echo_is_dropped_only_once() {
		let mut echoes = EchoMap::default();
		let path = PathBuf::from("src/init.luau");

		echoes.record(&path);

		assert!(echoes.consume(&path));
		assert!(!echoes.consume(&path));
	}

	#[test]
	fn echo_after_ttl_is_not_dropped() {
		let mut echoes = EchoMap::default();
		let path = PathBuf::from("src/init.luau");

		echoes.record(&path);
		expire();

		assert!(!echoes.consume(&path));
	}

	#[test]
	fn unrelated_path_is_never_dropped() {
		let mut echoes = EchoMap::default();

		echoes.record(&PathBuf::from("src/init.luau"));

		assert!(!echoes.consume(&PathBuf::from("src/other.luau")));
		// Recording one path must not suppress its parent either
		assert!(!echoes.consume(&PathBuf::from("src")));
	}

	#[test]
	fn expired_entries_are_evicted() {
		let mut echoes = EchoMap::default();

		for index in 0..100 {
			echoes.record(&PathBuf::from(format!("src/{index}.luau")));
		}

		assert_eq!(echoes.inner.len(), 100);

		expire();
		echoes.record(&PathBuf::from("src/init.luau"));

		assert_eq!(echoes.inner.len(), 1);
	}
}
