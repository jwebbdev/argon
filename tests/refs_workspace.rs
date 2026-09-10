//! Drives the whole read and write path over a real workspace, as the
//! interesting part of a reference is that it survives the trip from a
//! file into the tree and back into a file

use argon::{
	core::{
		meta::Meta,
		processor::write,
		refs,
		snapshot::{Snapshot, UpdatedSnapshot},
		tree::Tree,
	},
	middleware::new_snapshot,
	project::Project,
	vfs::Vfs,
	Properties,
};

use rbx_dom_weak::{
	types::{Ref, Variant},
	ustr, InstanceBuilder, WeakDom,
};

use std::{
	env, fs,
	path::PathBuf,
	sync::atomic::{AtomicUsize, Ordering},
};

const PROJECT: &str = r#"{
	"name": "RefTest",
	"tree": {
		"$className": "DataModel",
		"ReplicatedStorage": {
			"$path": "src"
		}
	}
}"#;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Workspace {
	path: PathBuf,
}

impl Workspace {
	fn new() -> Self {
		let path = env::temp_dir().join(format!(
			"argon-refs-{}-{}",
			std::process::id(),
			COUNTER.fetch_add(1, Ordering::Relaxed)
		));

		let _ = fs::remove_dir_all(&path);
		fs::create_dir_all(path.join("src")).unwrap();

		let workspace = Self { path };
		workspace.write("default.project.json", PROJECT);

		workspace
	}

	fn write(&self, path: &str, contents: &str) {
		let path = self.path.join(path);

		fs::create_dir_all(path.parent().unwrap()).unwrap();
		fs::write(path, contents).unwrap();
	}

	fn read(&self, path: &str) -> String {
		fs::read_to_string(self.path.join(path)).unwrap()
	}

	fn exists(&self, path: &str) -> bool {
		self.path.join(path).exists()
	}

	/// Builds the tree the same way `Core` does when a session starts
	fn tree(&self) -> Tree {
		let project = Project::load(&self.path.join("default.project.json")).unwrap();
		let vfs = Vfs::new(false);

		let meta = Meta::from_project(&project);
		let snapshot = new_snapshot(&project.path, &meta.context, &vfs).unwrap().unwrap();

		let mut tree = Tree::new(snapshot);
		refs::resolve_all(&mut tree);

		tree
	}
}

impl Drop for Workspace {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(&self.path);
	}
}

fn find(tree: &Tree, path: &[&str]) -> Ref {
	let mut current = tree.root_ref();

	for name in path {
		current = *tree
			.get_instance(current)
			.unwrap_or_else(|| panic!("{path:?} does not exist"))
			.children()
			.iter()
			.find(|child| tree.get_instance(**child).unwrap().name == *name)
			.unwrap_or_else(|| panic!("{path:?} does not exist"));
	}

	current
}

fn property(tree: &Tree, id: Ref, property: &str) -> Option<Variant> {
	tree.get_instance(id)?.properties.get(&ustr(property)).cloned()
}

fn write_rbxmx(workspace: &Workspace, path: &str) {
	let mut dom = WeakDom::new(InstanceBuilder::new("Model").with_name("Ship"));
	let model = dom.root_ref();

	let root = dom.insert(model, InstanceBuilder::new("Part").with_name("Root"));
	let effects = dom.insert(model, InstanceBuilder::new("Folder").with_name("Effects"));
	let beam = dom.insert(effects, InstanceBuilder::new("Beam").with_name("Trail"));

	dom.get_by_ref_mut(model)
		.unwrap()
		.properties
		.insert(ustr("PrimaryPart"), Variant::Ref(root));

	dom.get_by_ref_mut(beam)
		.unwrap()
		.properties
		.insert(ustr("Attachment0"), Variant::Ref(root));

	let mut contents = Vec::new();
	rbx_xml::to_writer_default(&mut contents, &dom, &[model]).unwrap();

	fs::create_dir_all(workspace.path.join(path).parent().unwrap()).unwrap();
	fs::write(workspace.path.join(path), contents).unwrap();
}

#[test]
fn reference_written_as_a_path_points_at_the_instance() {
	let workspace = Workspace::new();

	workspace.write(
		"src/Ship/init.meta.json",
		r#"{ "className": "Model", "properties": { "PrimaryPart": ".Root" } }"#,
	);
	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);

	let tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(root)));
}

#[test]
fn reference_reaching_into_another_file_points_at_the_instance() {
	let workspace = Workspace::new();

	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);
	workspace.write(
		"src/Ship/Effects/Trail.model.json",
		r#"{ "ClassName": "Beam", "Properties": { "Attachment0": "^^Root" } }"#,
	);

	let tree = workspace.tree();

	let trail = find(&tree, &["ReplicatedStorage", "Ship", "Effects", "Trail"]);
	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);

	assert_eq!(property(&tree, trail, "Attachment0"), Some(Variant::Ref(root)));
}

#[test]
fn absolute_reference_points_at_the_instance() {
	let workspace = Workspace::new();

	workspace.write("src/Root.model.json", r#"{ "ClassName": "Part" }"#);
	workspace.write(
		"src/Ship.model.json",
		r#"{ "ClassName": "Model", "Properties": { "PrimaryPart": "ReplicatedStorage.Root" } }"#,
	);

	let tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let root = find(&tree, &["ReplicatedStorage", "Root"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(root)));
}

#[test]
fn reference_pointing_at_nothing_is_reported_but_harmless() {
	let workspace = Workspace::new();

	workspace.write(
		"src/Ship.model.json",
		r#"{ "ClassName": "Model", "Properties": { "PrimaryPart": ".Missing" } }"#,
	);

	let tree = workspace.tree();
	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), None);
}

#[test]
fn references_of_a_model_file_survive_being_read() {
	let workspace = Workspace::new();
	write_rbxmx(&workspace, "src/Ship.rbxmx");

	let tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);
	let trail = find(&tree, &["ReplicatedStorage", "Ship", "Effects", "Trail"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(root)));
	assert_eq!(property(&tree, trail, "Attachment0"), Some(Variant::Ref(root)));
}

#[test]
fn reference_the_client_sets_is_written_as_a_path() {
	let workspace = Workspace::new();

	workspace.write("src/Ship/init.meta.json", r#"{ "className": "Model" }"#);
	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);

	let mut tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);

	let mut properties = Properties::default();
	properties.insert(ustr("PrimaryPart"), Variant::Ref(root));

	let mut update = UpdatedSnapshot::new(ship);
	update.properties = Some(properties);

	let vfs = Vfs::new(false);
	write::apply_update(update, &mut tree, &vfs).unwrap();

	let data = workspace.read("src/Ship/init.meta.json");

	assert!(
		data.contains(r#""PrimaryPart": ".Root""#),
		"expected a path in the data file, got {data}"
	);
	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(root)));

	// And the very same file has to read back into the very same tree
	let tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(root)));
}

#[test]
fn reference_the_client_clears_is_taken_out_of_the_file() {
	let workspace = Workspace::new();

	workspace.write(
		"src/Ship/init.meta.json",
		r#"{ "className": "Model", "properties": { "PrimaryPart": ".Root" } }"#,
	);
	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);

	let mut tree = workspace.tree();
	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);

	let mut update = UpdatedSnapshot::new(ship);
	update.properties = Some(Properties::default());

	let vfs = Vfs::new(false);
	write::apply_update(update, &mut tree, &vfs).unwrap();

	assert!(
		!workspace.exists("src/Ship/init.meta.json")
			|| !workspace.read("src/Ship/init.meta.json").contains("PrimaryPart"),
		"expected the reference to be gone, got {}",
		workspace.read("src/Ship/init.meta.json")
	);
}

#[test]
fn reference_pointing_outside_the_tree_is_not_written() {
	let workspace = Workspace::new();

	workspace.write("src/Ship/init.meta.json", r#"{ "className": "Model" }"#);
	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);

	let mut tree = workspace.tree();
	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);

	let mut properties = Properties::default();
	properties.insert(ustr("PrimaryPart"), Variant::Ref(Ref::new()));

	let mut update = UpdatedSnapshot::new(ship);
	update.properties = Some(properties);

	let vfs = Vfs::new(false);
	write::apply_update(update, &mut tree, &vfs).unwrap();

	assert!(
		!workspace.exists("src/Ship/init.meta.json")
			|| !workspace.read("src/Ship/init.meta.json").contains("PrimaryPart"),
		"expected no reference to be written, got {}",
		workspace.read("src/Ship/init.meta.json")
	);
}

#[test]
fn references_of_an_addition_are_written_as_paths() {
	let workspace = Workspace::new();
	workspace.write("src/.data.json", "{}");

	let mut tree = workspace.tree();
	let parent = find(&tree, &["ReplicatedStorage"]);

	let root_id = Ref::new();

	let root = Snapshot::new().with_id(root_id).with_name("Root").with_class("Part");

	let mut trail = Snapshot::new()
		.with_id(Ref::new())
		.with_name("Trail")
		.with_class("Beam");
	trail.add_property("Attachment0", Variant::Ref(root_id));

	let mut ship = Snapshot::new()
		.with_id(Ref::new())
		.with_name("Ship")
		.with_class("Model")
		.with_children(vec![root, trail]);

	ship.add_property("PrimaryPart", Variant::Ref(root_id));

	let vfs = Vfs::new(false);
	write::apply_addition(ship.as_new(parent), &mut tree, &vfs).unwrap();

	let data = workspace.read("src/Ship/init.meta.json");
	assert!(
		data.contains(r#""PrimaryPart": ".Root""#),
		"expected a path in the data file, got {data}"
	);

	let trail = workspace.read("src/Ship/Trail/init.meta.json");
	assert!(
		trail.contains(r#""Attachment0": "^Root""#),
		"expected a path in the data file, got {trail}"
	);

	// Reading the very same files back has to rebuild the very same links
	let tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);
	let trail = find(&tree, &["ReplicatedStorage", "Ship", "Trail"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(root)));
	assert_eq!(property(&tree, trail, "Attachment0"), Some(Variant::Ref(root)));
}

#[test]
fn renaming_a_target_rewrites_the_files_pointing_at_it() {
	let workspace = Workspace::new();

	workspace.write(
		"src/Ship/init.meta.json",
		r#"{ "className": "Model", "properties": { "PrimaryPart": ".Root" } }"#,
	);
	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);

	let mut tree = workspace.tree();

	let root = find(&tree, &["ReplicatedStorage", "Ship", "Root"]);

	let mut rename = UpdatedSnapshot::new(root);
	rename.name = Some("Base".to_owned());

	let vfs = Vfs::new(false);

	write::apply_update(rename, &mut tree, &vfs).unwrap();
	write::refresh_refs(&mut tree, &vfs).unwrap();

	let data = workspace.read("src/Ship/init.meta.json");
	assert!(
		data.contains(r#""PrimaryPart": ".Base""#),
		"expected the path to follow the rename, got {data}"
	);

	// And the tree the files rebuild has to keep the very same link
	let tree = workspace.tree();

	let ship = find(&tree, &["ReplicatedStorage", "Ship"]);
	let base = find(&tree, &["ReplicatedStorage", "Ship", "Base"]);

	assert_eq!(property(&tree, ship, "PrimaryPart"), Some(Variant::Ref(base)));
}

#[test]
fn a_path_nobody_could_resolve_is_left_alone() {
	let workspace = Workspace::new();

	workspace.write(
		"src/Ship/init.meta.json",
		r#"{ "className": "Model", "properties": { "PrimaryPart": ".Typo" } }"#,
	);
	workspace.write("src/Ship/Root.model.json", r#"{ "ClassName": "Part" }"#);

	let mut tree = workspace.tree();
	let vfs = Vfs::new(false);

	write::refresh_refs(&mut tree, &vfs).unwrap();

	let data = workspace.read("src/Ship/init.meta.json");
	assert!(
		data.contains(".Typo"),
		"expected the path to be left for its author to fix, got {data}"
	);
}
