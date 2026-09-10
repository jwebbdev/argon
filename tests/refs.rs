mod ref_path {
	use argon::{
		core::{
			refs::{self, RefPath},
			snapshot::Snapshot,
			tree::Tree,
		},
		Properties,
	};

	use rbx_dom_weak::{
		types::{Ref, Variant},
		ustr, UstrMap,
	};

	fn instance(name: &str, children: Vec<Snapshot>) -> Snapshot {
		Snapshot::new()
			.with_name(name)
			.with_class("Folder")
			.with_children(children)
	}

	/// Builds a tree that holds `Workspace.Model` with a `Root` and an
	/// `Effects.Beam` inside of it, next to a `Lighting` service
	fn tree() -> Tree {
		Tree::new(
			Snapshot::new()
				.with_name("game")
				.with_class("DataModel")
				.with_children(vec![
					instance(
						"Workspace",
						vec![instance(
							"Model",
							vec![
								instance("Root", vec![]),
								instance("Effects", vec![instance("Beam", vec![])]),
							],
						)],
					),
					instance("Lighting", vec![]),
				]),
		)
	}

	fn find(tree: &Tree, path: &[&str]) -> Ref {
		let mut current = tree.root_ref();

		for name in path {
			current = *tree
				.get_instance(current)
				.unwrap()
				.children()
				.iter()
				.find(|child| tree.get_instance(**child).unwrap().name == *name)
				.unwrap();
		}

		current
	}

	#[test]
	fn absolute_path_round_trips() {
		let path = RefPath::parse("Workspace.Model.Root").unwrap();

		assert_eq!(
			path,
			RefPath::Absolute(vec!["Workspace".into(), "Model".into(), "Root".into()])
		);
		assert_eq!(path.to_string(), "Workspace.Model.Root");
	}

	#[test]
	fn relative_path_round_trips() {
		let path = RefPath::parse("^^Effects.Beam").unwrap();

		assert_eq!(
			path,
			RefPath::Relative {
				up: 2,
				down: vec!["Effects".into(), "Beam".into()],
			}
		);
		assert_eq!(path.to_string(), "^^Effects.Beam");
	}

	#[test]
	fn path_to_an_ancestor_round_trips() {
		let path = RefPath::parse("^^").unwrap();

		assert_eq!(
			path,
			RefPath::Relative {
				up: 2,
				down: Vec::new()
			}
		);
		assert_eq!(path.to_string(), "^^");
	}

	#[test]
	fn id_round_trips() {
		let path = RefPath::parse("#hull").unwrap();

		assert_eq!(path, RefPath::Id("hull".into()));
		assert_eq!(path.to_string(), "#hull");
	}

	#[test]
	fn empty_path_points_at_nothing() {
		assert_eq!(RefPath::parse(""), None);
		assert_eq!(RefPath::parse("#"), None);
	}

	#[test]
	fn id_resolves_wherever_the_instance_is() {
		let mut tree = tree();
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let mut meta = tree.get_meta(root).unwrap().clone();
		meta.set_id(Some("hull".into()));
		tree.update_meta(root, meta);

		let lighting = find(&tree, &["Lighting"]);
		let path = RefPath::parse("#hull").unwrap();

		assert_eq!(path.resolve(&tree, lighting), Some(root));
	}

	#[test]
	fn an_id_is_preferred_over_a_path() {
		let mut tree = tree();

		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		assert_eq!(RefPath::between(&tree, beam, root).unwrap().to_string(), "^^Root");

		let mut meta = tree.get_meta(root).unwrap().clone();
		meta.set_id(Some("hull".into()));
		tree.update_meta(root, meta);

		assert_eq!(RefPath::between(&tree, beam, root).unwrap().to_string(), "#hull");
	}

	#[test]
	fn an_id_that_nothing_claims_points_at_nothing() {
		let tree = tree();
		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);

		assert_eq!(RefPath::parse("#hull").unwrap().resolve(&tree, beam), None);
	}

	#[test]
	fn absolute_path_resolves_from_the_root() {
		let tree = tree();

		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let path = RefPath::parse("Workspace.Model.Root").unwrap();

		assert_eq!(path.resolve(&tree, beam), Some(root));
	}

	#[test]
	fn relative_path_resolves_from_the_instance() {
		let tree = tree();

		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let path = RefPath::parse("^^Root").unwrap();

		assert_eq!(path.resolve(&tree, beam), Some(root));
	}

	#[test]
	fn path_that_walks_past_the_root_points_at_nothing() {
		let tree = tree();
		let lighting = find(&tree, &["Lighting"]);

		let path = RefPath::parse("^^^^^Workspace").unwrap();

		assert_eq!(path.resolve(&tree, lighting), None);
	}

	#[test]
	fn path_that_walks_onto_the_root_itself_points_at_nothing() {
		let tree = tree();
		let lighting = find(&tree, &["Lighting"]);

		let path = RefPath::parse("^^").unwrap();

		assert_eq!(path.resolve(&tree, lighting), None);
	}

	#[test]
	fn path_between_two_instances_stays_relative() {
		let tree = tree();

		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let path = RefPath::between(&tree, beam, root).unwrap();

		assert_eq!(path.to_string(), "^^Root");
		assert_eq!(path.resolve(&tree, beam), Some(root));
	}

	#[test]
	fn path_between_distant_instances_walks_up_to_the_root() {
		let tree = tree();

		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);
		let lighting = find(&tree, &["Lighting"]);

		let path = RefPath::between(&tree, beam, lighting).unwrap();

		assert_eq!(path.to_string(), "^^^^Lighting");
		assert_eq!(path.resolve(&tree, beam), Some(lighting));
	}

	#[test]
	fn path_between_an_instance_and_itself_resolves_back_to_it() {
		let tree = tree();
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let path = RefPath::between(&tree, root, root).unwrap();

		assert_eq!(path.resolve(&tree, root), Some(root));
	}

	#[test]
	fn path_to_an_ancestor_resolves_to_it() {
		let tree = tree();

		let beam = find(&tree, &["Workspace", "Model", "Effects", "Beam"]);
		let model = find(&tree, &["Workspace", "Model"]);

		let path = RefPath::between(&tree, beam, model).unwrap();

		assert_eq!(path.to_string(), "^^");
		assert_eq!(path.resolve(&tree, beam), Some(model));
	}

	#[test]
	fn resolving_points_the_instance_at_its_target() {
		let mut tree = tree();

		let model = find(&tree, &["Workspace", "Model"]);
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let mut meta = tree.get_meta(model).unwrap().clone();
		meta.refs.insert(ustr("PrimaryPart"), RefPath::parse(".Root").unwrap());
		tree.update_meta(model, meta);

		let updates = refs::resolve_all(&mut tree);

		assert_eq!(updates.len(), 1);
		assert_eq!(updates[0].id, model);
		assert_eq!(
			tree.get_instance(model).unwrap().properties.get(&ustr("PrimaryPart")),
			Some(&Variant::Ref(root))
		);
	}

	#[test]
	fn resolving_twice_reports_no_further_changes() {
		let mut tree = tree();
		let model = find(&tree, &["Workspace", "Model"]);

		let mut meta = tree.get_meta(model).unwrap().clone();
		meta.refs.insert(ustr("PrimaryPart"), RefPath::parse(".Root").unwrap());
		tree.update_meta(model, meta);

		assert_eq!(refs::resolve_all(&mut tree).len(), 1);
		assert_eq!(refs::resolve_all(&mut tree).len(), 0);
	}

	#[test]
	fn unresolvable_reference_is_left_alone() {
		let mut tree = tree();
		let model = find(&tree, &["Workspace", "Model"]);

		let mut meta = tree.get_meta(model).unwrap().clone();
		meta.refs
			.insert(ustr("PrimaryPart"), RefPath::parse(".Missing").unwrap());
		tree.update_meta(model, meta);

		assert_eq!(refs::resolve_all(&mut tree).len(), 0);
		assert_eq!(
			tree.get_instance(model).unwrap().properties.get(&ustr("PrimaryPart")),
			None
		);
	}

	#[test]
	fn instance_without_references_is_never_visited() {
		let mut tree = tree();

		assert!(tree.ids_with_refs().is_empty());
		assert_eq!(refs::resolve_all(&mut tree).len(), 0);
	}

	#[test]
	fn describing_properties_produces_a_resolvable_path() {
		let tree = tree();

		let model = find(&tree, &["Workspace", "Model"]);
		let root = find(&tree, &["Workspace", "Model", "Root"]);

		let mut properties = Properties::default();
		properties.insert(ustr("PrimaryPart"), Variant::Ref(root));

		let paths = refs::from_properties(&properties, &tree, model);

		assert_eq!(
			paths.get(&ustr("PrimaryPart")).map(|path| path.to_string()),
			Some(".Root".into())
		);

		let properties = refs::to_paths(properties, &paths);

		assert_eq!(
			properties.get(&ustr("PrimaryPart")),
			Some(&Variant::String(".Root".into()))
		);
	}

	#[test]
	fn reference_with_no_known_path_is_dropped() {
		let mut properties = Properties::default();
		properties.insert(ustr("PrimaryPart"), Variant::Ref(Ref::new()));

		let properties = refs::to_paths(properties, &UstrMap::default());

		assert!(properties.is_empty());
	}

	#[test]
	fn references_added_by_the_client_are_described_by_the_snapshot() {
		let tree = tree();

		let model_id = Ref::new();
		let root_id = Ref::new();
		let beam_id = Ref::new();

		let mut root = instance("Root", vec![]).with_id(root_id);
		let mut beam = instance("Beam", vec![]).with_id(beam_id);

		beam.add_property("Attachment0", Variant::Ref(root_id));
		root.add_property("Attachment0", Variant::Ref(find(&tree, &["Lighting"])));

		let mut snapshot = instance("Model", vec![root, beam]).with_id(model_id);
		snapshot.add_property("PrimaryPart", Variant::Ref(root_id));

		refs::from_snapshot(&mut snapshot, &tree);

		assert_eq!(
			snapshot
				.meta
				.refs
				.get(&ustr("PrimaryPart"))
				.map(|path| path.to_string()),
			Some(".Root".into())
		);
		assert_eq!(
			snapshot.children[1]
				.meta
				.refs
				.get(&ustr("Attachment0"))
				.map(|path| path.to_string()),
			Some("^Root".into())
		);
		assert_eq!(
			snapshot.children[0]
				.meta
				.refs
				.get(&ustr("Attachment0"))
				.map(|path| path.to_string()),
			Some("Lighting".into())
		);
	}
}

mod unresolved_value {
	use argon::{core::refs::RefPath, resolution::UnresolvedValue};

	fn unresolved(value: &str) -> UnresolvedValue {
		serde_json::from_str(value).unwrap()
	}

	#[test]
	fn reference_property_is_read_as_a_path() {
		let value = unresolved(r#""Workspace.Model.Root""#);

		assert_eq!(
			value.to_ref_path("Model", "PrimaryPart"),
			RefPath::parse("Workspace.Model.Root")
		);
	}

	#[test]
	fn relative_reference_property_is_read_as_a_path() {
		let value = unresolved(r#""^^Root""#);

		assert_eq!(value.to_ref_path("Beam", "Attachment0"), RefPath::parse("^^Root"));
	}

	#[test]
	fn empty_reference_property_points_at_nothing() {
		assert_eq!(unresolved(r#""""#).to_ref_path("Model", "PrimaryPart"), None);
	}

	#[test]
	fn plain_property_is_not_read_as_a_path() {
		assert_eq!(unresolved(r#""Baseplate""#).to_ref_path("Model", "Name"), None);
	}

	#[test]
	fn reference_cannot_be_resolved_on_its_own() {
		assert!(unresolved(r#""Workspace.Model.Root""#)
			.resolve("Model", "PrimaryPart")
			.is_err());
	}
}

// A file system cannot hold two siblings of the same name, but a Roblox tree
// can, and Argon keeps them around when `keep_duplicates` is on. A path has no
// way of telling those apart, so it must refuse rather than guess
mod ambiguous_names {
	use argon::core::{refs::RefPath, snapshot::Snapshot, tree::Tree};
	use rbx_dom_weak::types::Ref;

	fn tree_with_duplicates() -> (Tree, Ref, Ref, Vec<Ref>) {
		let tree = Tree::new(Snapshot::new().with_class("Folder").with_children(vec![
			Snapshot::new().with_class("Model").with_name("Rig").with_children(vec![
				Snapshot::new().with_class("Part").with_name("Part"),
				Snapshot::new().with_class("Part").with_name("Part"),
				Snapshot::new().with_class("Part").with_name("Only"),
			]),
			Snapshot::new().with_class("ObjectValue").with_name("Link"),
		]));

		let root = tree.root_ref();
		let rig = tree.get_instance(root).unwrap().children()[0];
		let link = tree.get_instance(root).unwrap().children()[1];
		let parts = tree.get_instance(rig).unwrap().children().to_vec();

		(tree, rig, link, parts)
	}

	#[test]
	fn refuses_to_describe_an_instance_with_a_duplicate_name() {
		let (tree, _, link, parts) = tree_with_duplicates();

		assert_eq!(
			RefPath::between(&tree, link, parts[0]),
			None,
			"one of two instances named `Part` cannot be told apart by a path"
		);
		assert_eq!(RefPath::between(&tree, link, parts[1]), None);
	}

	#[test]
	fn still_describes_an_instance_whose_name_is_its_own() {
		let (tree, _, link, parts) = tree_with_duplicates();

		let path = RefPath::between(&tree, link, parts[2]).expect("`Only` is the only one of its name");

		assert_eq!(
			path.resolve(&tree, link),
			Some(parts[2]),
			"an unambiguous name has to survive the round trip"
		);
	}

	#[test]
	fn refuses_to_resolve_a_path_that_more_than_one_instance_answers_to() {
		let (tree, _, link, _) = tree_with_duplicates();

		let path = RefPath::parse("^Rig.Part").expect("a written path is always readable");

		assert_eq!(
			path.resolve(&tree, link),
			None,
			"resolving to whichever came first would point at the wrong instance"
		);
	}

	#[test]
	fn an_id_still_works_where_a_name_cannot() {
		let mut duplicate = Snapshot::new().with_class("Part").with_name("Part");
		duplicate.meta.id = Some(String::from("the-one-i-meant"));

		let tree = Tree::new(Snapshot::new().with_class("Folder").with_children(vec![
				Snapshot::new()
					.with_class("Model")
					.with_name("Rig")
					.with_children(vec![Snapshot::new().with_class("Part").with_name("Part"), duplicate]),
				Snapshot::new().with_class("ObjectValue").with_name("Link"),
			]));

		let root = tree.root_ref();
		let rig = tree.get_instance(root).unwrap().children()[0];
		let link = tree.get_instance(root).unwrap().children()[1];
		let target = tree.get_instance(rig).unwrap().children()[1];

		let path = RefPath::between(&tree, link, target).expect("an id needs no name at all");

		assert_eq!(path.to_string(), "#the-one-i-meant");
		assert_eq!(path.resolve(&tree, link), Some(target));
	}
}
