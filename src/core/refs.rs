use log::warn;
use rbx_dom_weak::{
	types::{Ref, Variant},
	AHashMap, UstrMap,
};
use std::fmt::{self, Display, Formatter};

use super::{
	snapshot::{Snapshot, UpdatedSnapshot},
	tree::Tree,
};
use crate::Properties;

/// Separates the individual instance names of a [`RefPath`]
const SEPARATOR: char = '.';
/// Prefixes a [`RefPath`] with one hop towards the root of the tree
const HOP: char = '^';
/// Marks a [`RefPath`] as the id an instance gave itself
const ID: char = '#';

/// The location of an instance that another instance points at, kept
/// as a path instead of a [`Ref`] because references have to survive
/// on disk, where the ids of a single Argon session mean nothing
///
/// Absolute paths are written the way `Instance:GetFullName` returns
/// them, so `Workspace.Model.Root` refers to the instance named `Root`
/// inside of the `Model` that lives in `Workspace`
///
/// Relative paths begin with one `^` for every hop towards the root
/// that has to be made first, so `^^Attachments.A0` refers to
/// `Attachments.A0` inside of the grandparent of the instance that
/// holds the reference. The ones that need no hop at all begin with a
/// single `.` instead, the way `.Root` refers to a child named `Root`
///
/// An instance can also give itself a name for others to point at, by
/// putting an `id` in its data file. `#root` refers to whichever
/// instance calls itself `root`, no matter where it lives or what it
/// is called, which is what makes it worth writing by hand
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPath {
	Id(String),
	Absolute(Vec<String>),
	Relative { up: usize, down: Vec<String> },
}

impl RefPath {
	/// Reads a path back from its textual form, returning `None` if it
	/// is empty and thus does not point at any instance at all
	pub fn parse(path: &str) -> Option<Self> {
		if let Some(id) = path.strip_prefix(ID) {
			if id.is_empty() {
				return None;
			}

			return Some(Self::Id(id.to_owned()));
		}

		let up = path.chars().take_while(|char| *char == HOP).count();

		let (relative, rest) = if up > 0 {
			(true, &path[up..])
		} else if let Some(rest) = path.strip_prefix(SEPARATOR) {
			(true, rest)
		} else {
			(false, path)
		};

		let down: Vec<String> = if rest.is_empty() {
			Vec::new()
		} else {
			rest.split(SEPARATOR).map(|name| name.to_owned()).collect()
		};

		if !relative {
			if down.is_empty() {
				return None;
			}

			return Some(Self::Absolute(down));
		}

		Some(Self::Relative { up, down })
	}

	/// Describes where `target` is, as seen from `from`
	///
	/// The path stays relative to the closest instance both of them
	/// live under, so that moving or renaming anything above it does
	/// not turn the reference into a dangling one
	pub fn between(tree: &Tree, from: Ref, target: Ref) -> Option<Self> {
		// An id survives the instance being renamed or moved, so it is
		// always the better way to describe where it is
		if let Some(id) = tree.get_meta(target).and_then(|meta| meta.id.as_ref()) {
			return Some(Self::Id(id.to_owned()));
		}

		if tree.get_instance(from).is_none() {
			return Self::absolute(tree, target);
		}

		if from == target {
			return Some(Self::Relative {
				up: 1,
				down: vec![unambiguous_name(tree, from)?],
			});
		}

		let from_ancestors = ancestors(tree, from)?;
		let target_ancestors = ancestors(tree, target)?;

		let up = from_ancestors.iter().position(|id| target_ancestors.contains(id))?;
		let common = from_ancestors[up];

		let down = target_ancestors
			.iter()
			.take_while(|id| **id != common)
			.map(|id| unambiguous_name(tree, *id))
			.collect::<Option<Vec<String>>>()?
			.into_iter()
			.rev()
			.collect();

		Some(Self::Relative { up, down })
	}

	/// Describes where `target` is, as seen from the root of the tree
	pub fn absolute(tree: &Tree, target: Ref) -> Option<Self> {
		let mut names: Vec<String> = ancestors(tree, target)?
			.iter()
			.take_while(|id| **id != tree.root_ref())
			.map(|id| unambiguous_name(tree, *id))
			.collect::<Option<Vec<String>>>()?;

		if names.is_empty() {
			return None;
		}

		names.reverse();

		Some(Self::Absolute(names))
	}

	/// Looks up the instance this path points at, starting from `from`
	/// when the path is relative
	pub fn resolve(&self, tree: &Tree, from: Ref) -> Option<Ref> {
		let (mut current, down) = match self {
			Self::Id(id) => return tree.get_by_ref_id(id),
			Self::Absolute(down) => (tree.root_ref(), down),
			Self::Relative { up, down } => {
				let mut current = from;

				for _ in 0..*up {
					current = tree.get_instance(current)?.parent();
				}

				// Walking past the root lands on the parent of it, which
				// is no instance at all
				if current.is_none() {
					return None;
				}

				(current, down)
			}
		};

		for name in down {
			let mut matches = tree
				.get_instance(current)?
				.children()
				.iter()
				.copied()
				.filter(|child| tree.get_instance(*child).is_some_and(|child| child.name == *name));

			current = matches.next()?;

			// Picking the first of several instances that all answer to the
			// same name would quietly point the reference at the wrong one
			if matches.next().is_some() {
				warn!(
					"Reference to {name} is ambiguous as more than one instance of that name exists. 					 Give the one that is meant an `id` and point at that instead"
				);

				return None;
			}
		}

		Some(current)
	}
}

impl Display for RefPath {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		let (up, down) = match self {
			Self::Id(id) => return write!(f, "{ID}{id}"),
			Self::Absolute(down) => (0, down),
			Self::Relative { up, down } => (*up, down),
		};

		if let Self::Relative { up: 0, .. } = self {
			write!(f, "{SEPARATOR}")?;
		}

		for _ in 0..up {
			write!(f, "{HOP}")?;
		}

		let mut down = down.iter();

		if let Some(name) = down.next() {
			write!(f, "{name}")?;
		}

		for name in down {
			write!(f, "{SEPARATOR}{name}")?;
		}

		Ok(())
	}
}

/// Returns the name of an instance, but only when no sibling of it answers
/// to the same name
///
/// A shared name cannot be written into a path, as there would be no way of
/// telling which of them was meant when reading it back. Instances like that
/// have to be given an `id` to be referenced at all
fn unambiguous_name(tree: &Tree, id: Ref) -> Option<String> {
	let instance = tree.get_instance(id)?;
	let name = instance.name.clone();

	let siblings = tree
		.get_instance(instance.parent())
		.map(|parent| {
			parent
				.children()
				.iter()
				.filter(|child| tree.get_instance(**child).is_some_and(|child| child.name == name))
				.count()
		})
		.unwrap_or(1);

	if siblings > 1 {
		warn!(
			"Instance {name} shares its name with a sibling so it cannot be described by a path. 			 Give it an `id` to be able to reference it"
		);

		return None;
	}

	Some(name)
}

/// Lists the instance and all of its ancestors, ending with the root
fn ancestors(tree: &Tree, id: Ref) -> Option<Vec<Ref>> {
	let root = tree.root_ref();
	let mut ancestors = vec![id];
	let mut current = id;

	tree.get_instance(id)?;

	while current != root {
		current = tree.get_instance(current)?.parent();

		if current.is_none() {
			return None;
		}

		ancestors.push(current);
	}

	Some(ancestors)
}

/// Points every instance that holds a [`RefPath`] at the instance that
/// path describes, returning the updates that have to be sent to the
/// client for the ones whose target changed
///
/// This runs after the tree has been built, as the instance a path
/// points at is often read from a completely different file than the
/// one that holds the reference, and might not exist yet at the time
/// the reference itself is read
pub fn resolve_all(tree: &mut Tree) -> Vec<UpdatedSnapshot> {
	let mut updates = Vec::new();

	let unresolved: Vec<(Ref, UstrMap<RefPath>)> = tree
		.ids_with_refs()
		.iter()
		.filter_map(|id| Some((*id, tree.get_meta(*id)?.refs.clone())))
		.collect();

	for (id, refs) in unresolved {
		let mut resolved = Properties::default();

		for (property, path) in &refs {
			let Some(target) = path.resolve(tree, id) else {
				warn!("Failed to resolve reference {path} of {property} property, instance: {id:?}");
				continue;
			};

			let target = Variant::Ref(target);

			if tree
				.get_instance(id)
				.is_some_and(|instance| instance.properties.get(property) == Some(&target))
			{
				continue;
			}

			resolved.insert(*property, target);
		}

		if resolved.is_empty() {
			continue;
		}

		let Some(instance) = tree.get_instance_mut(id) else {
			continue;
		};

		instance.properties.extend(resolved);

		let mut update = UpdatedSnapshot::new(id);
		update.properties = Some(instance.properties.clone());

		updates.push(update);
	}

	updates
}

/// Swaps every reference for the path of the instance it points at, so
/// that it survives being written to a file and read back later
///
/// References with no known path are dropped, as a raw id would mean
/// nothing in another session
pub fn to_paths(properties: Properties, refs: &UstrMap<RefPath>) -> Properties {
	properties
		.into_iter()
		.filter_map(|(property, variant)| {
			if !matches!(variant, Variant::Ref(_)) {
				return Some((property, variant));
			}

			let path = refs.get(&property)?;

			Some((property, Variant::String(path.to_string())))
		})
		.collect()
}

/// Describes every reference the snapshot and its descendants hold as
/// a path, using the snapshot itself for the ones that point inside of
/// it and the tree for the ones that point at an instance outside
///
/// Instances added by the client all arrive at once, so the ones they
/// point at each other with are not part of the tree yet
pub fn from_snapshot(snapshot: &mut Snapshot, tree: &Tree) {
	fn collect(snapshot: &Snapshot, path: Vec<String>, paths: &mut AHashMap<Ref, Vec<String>>) {
		for child in &snapshot.children {
			let mut child_path = path.clone();
			child_path.push(child.name.clone());

			collect(child, child_path, paths);
		}

		paths.insert(snapshot.id, path);
	}

	fn walk(snapshot: &mut Snapshot, tree: &Tree, paths: &AHashMap<Ref, Vec<String>>, depth: usize) {
		let mut refs = UstrMap::default();

		for (property, variant) in &snapshot.properties {
			let Variant::Ref(target) = variant else {
				continue;
			};

			let id = tree.get_meta(*target).and_then(|meta| meta.id.as_ref());

			let path = match (id, paths.get(target)) {
				(Some(id), _) => Some(RefPath::Id(id.to_owned())),
				(None, Some(path)) => Some(RefPath::Relative {
					up: depth,
					down: path.clone(),
				}),
				(None, None) => RefPath::between(tree, snapshot.id, *target),
			};

			if let Some(path) = path {
				refs.insert(*property, path);
			}
		}

		snapshot.meta.refs.extend(refs);

		for child in &mut snapshot.children {
			walk(child, tree, paths, depth + 1);
		}
	}

	let mut paths = AHashMap::default();
	collect(snapshot, Vec::new(), &mut paths);

	walk(snapshot, tree, &paths, 0);
}

/// Describes every reference the given properties hold as a path, for
/// an instance that is already part of the tree
pub fn from_properties(properties: &Properties, tree: &Tree, id: Ref) -> UstrMap<RefPath> {
	let mut refs = UstrMap::default();

	for (property, variant) in properties {
		let Variant::Ref(target) = variant else {
			continue;
		};

		if let Some(path) = RefPath::between(tree, id, *target) {
			refs.insert(*property, path);
		}
	}

	refs
}
