use rbx_dom_weak::{
	types::{Ref, Variant},
	AHashMap, Instance, UstrMap, WeakDom,
};

use crate::core::{meta::Meta, refs::RefPath, snapshot::Snapshot};

// Based on Rojo's InstanceSnapshot::from_tree (https://github.com/rojo-rbx/rojo/blob/master/src/snapshot/instance_snapshot.rs#L105)
pub fn snapshot_from_dom(dom: WeakDom, id: Ref) -> Snapshot {
	let paths = ref_paths(&dom, id);
	let (_, mut raw_dom) = dom.into_raw();

	fn walk(
		id: Ref,
		raw_dom: &mut AHashMap<Ref, Instance>,
		paths: &AHashMap<Ref, Vec<String>>,
		depth: usize,
	) -> Snapshot {
		let mut instance = raw_dom
			.remove(&id)
			.expect("Provided ID does not exist in the current DOM");

		let children = instance
			.children()
			.iter()
			.map(|&child_id| walk(child_id, raw_dom, paths, depth + 1))
			.collect();

		let mut meta = Meta::new();

		if instance.class == "MeshPart" {
			meta.set_mesh_source(super::save_mesh(&instance.properties));
		}

		// The referents of the file this instance came from mean nothing
		// outside of it, so they get swapped for the path of whatever they
		// point at and resolved again once the instance is in the tree
		let mut refs = UstrMap::default();

		instance.properties.retain(|property, value| {
			let Variant::Ref(target) = value else {
				return true;
			};

			if let Some(path) = paths.get(target) {
				refs.insert(
					*property,
					RefPath::Relative {
						up: depth,
						down: path.clone(),
					},
				);
			}

			false
		});

		meta.set_refs(refs);

		Snapshot::new()
			.with_meta(meta)
			.with_name(&instance.name)
			.with_class(&instance.class)
			.with_properties(instance.properties)
			.with_children(children)
	}

	walk(id, &mut raw_dom, &paths, 0)
}

/// Maps every instance of the subtree to its path relative to the root
/// of that subtree, so that references inside of a single file can be
/// described without relying on where the file ends up in the tree
fn ref_paths(dom: &WeakDom, id: Ref) -> AHashMap<Ref, Vec<String>> {
	fn walk(dom: &WeakDom, id: Ref, path: Vec<String>, paths: &mut AHashMap<Ref, Vec<String>>) {
		let Some(instance) = dom.get_by_ref(id) else {
			return;
		};

		for child_id in instance.children() {
			let Some(child) = dom.get_by_ref(*child_id) else {
				continue;
			};

			let mut child_path = path.clone();
			child_path.push(child.name.clone());

			walk(dom, *child_id, child_path, paths);
		}

		paths.insert(id, path);
	}

	let mut paths = AHashMap::default();
	walk(dom, id, Vec::new(), &mut paths);

	paths
}

#[cfg(test)]
mod tests {
	use rbx_dom_weak::{ustr, InstanceBuilder};

	use super::*;

	fn model() -> (WeakDom, Ref, Ref, Ref) {
		let mut dom = WeakDom::new(InstanceBuilder::new("Model").with_name("Model"));
		let model = dom.root_ref();

		let root = dom.insert(model, InstanceBuilder::new("Part").with_name("Root"));
		let effects = dom.insert(model, InstanceBuilder::new("Folder").with_name("Effects"));
		let beam = dom.insert(effects, InstanceBuilder::new("Beam").with_name("Beam"));

		(dom, model, root, beam)
	}

	#[test]
	fn references_inside_a_file_become_relative_paths() {
		let (mut dom, model, root, beam) = model();

		dom.get_by_ref_mut(model)
			.unwrap()
			.properties
			.insert(ustr("PrimaryPart"), Variant::Ref(root));

		dom.get_by_ref_mut(beam)
			.unwrap()
			.properties
			.insert(ustr("Attachment0"), Variant::Ref(root));

		let snapshot = snapshot_from_dom(dom, model);

		assert_eq!(
			snapshot.meta.refs.get(&ustr("PrimaryPart")).map(RefPath::to_string),
			Some(".Root".to_owned())
		);
		assert!(!snapshot.properties.contains_key(&ustr("PrimaryPart")));

		let effects = snapshot.children.iter().find(|child| child.name == "Effects").unwrap();
		let beam = &effects.children[0];

		assert_eq!(
			beam.meta.refs.get(&ustr("Attachment0")).map(RefPath::to_string),
			Some("^^Root".to_owned())
		);
		assert!(!beam.properties.contains_key(&ustr("Attachment0")));
	}

	#[test]
	fn reference_leaving_a_file_is_dropped() {
		let (mut dom, model, ..) = model();

		dom.get_by_ref_mut(model)
			.unwrap()
			.properties
			.insert(ustr("PrimaryPart"), Variant::Ref(Ref::new()));

		let snapshot = snapshot_from_dom(dom, model);

		assert!(snapshot.meta.refs.is_empty());
		assert!(!snapshot.properties.contains_key(&ustr("PrimaryPart")));
	}
}
