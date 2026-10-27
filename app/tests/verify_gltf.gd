extends SceneTree
## Re-imports an exported GLB/glTF (Godot's glTF importer) or FBX (Godot's ufbx-based
## importer) and reports what it finds.
## Run: godot --headless --path app --script res://tests/verify_gltf.gd -- <file>

func _init() -> void:
	var path: String = OS.get_cmdline_user_args()[0]
	var fbx := path.get_extension().to_lower() == "fbx"
	var doc: GLTFDocument = FBXDocument.new() if fbx else GLTFDocument.new()
	var state: GLTFState = FBXState.new() if fbx else GLTFState.new()
	var err := doc.append_from_file(path, state)
	if err != OK:
		print("VERIFY import_error=", error_string(err))
		quit(1)
		return
	var root := doc.generate_scene(state)
	var meshes := 0
	var lines := 0
	var tris := 0
	var stack: Array[Node] = [root]
	while not stack.is_empty():
		var n: Node = stack.pop_back()
		stack.append_array(n.get_children())
		if n is ImporterMeshInstance3D or n is MeshInstance3D:
			var m = n.mesh
			for s in m.get_surface_count():
				var prim: int = m.surface_get_primitive_type(s)
				var arrays: Array = m.surface_get_arrays(s)
				if prim == Mesh.PRIMITIVE_TRIANGLES:
					meshes += 1
					tris += arrays[Mesh.ARRAY_INDEX].size() / 3
				else:
					lines += 1
	var mats := []
	for m in state.get_materials():
		mats.append("%s %s" % [m.resource_name, m.albedo_color.to_html(false)])
	print("VERIFY ", path.get_file(), " meshes=", meshes, " line_strips=", lines, " triangles=", tris, " materials=", mats)
	root.free()
	quit(0)
