extends SceneTree
## Builds the showcase maps into ready to play scenes (the environment comes from worldspawn keys).
## godot --headless --path godot --script res://tests/build_showcase.gd [-- map_name ...]
## Names after -- build only those maps, the others keep their saved scenes.
## A map with a Godot overlay scene (res://demo/overlays/<map>_overlay.tscn) gets it on top, instanced before the build
## so the build keeps it, and its overlay sidecar is written next to the map for TrenchDot.

const SETTINGS := "res://demo/demo_map_settings.tres"
const MAPS := ["mountain_house", "church_school", "lighthouse_forest", "withered_city", "night_district", "terrain3d_valley"]
## Maps whose ground is a Terrain3D node, skipped when Terrain3D is not installed, with the assets their node gets.
const TERRAIN3D_ASSETS := { "terrain3d_valley": "res://terrain3d_valley/terrain3d_assets.tres" }

func reown(node: Node, old_owner: Node, new_owner: Node) -> void:
	for child in node.get_children():
		if child.owner == old_owner or child.owner == null:
			child.owner = new_owner
		reown(child, old_owner, new_owner)

func count(node: Node, predicate: Callable) -> int:
	var n := 1 if predicate.call(node) else 0
	for c in node.get_children():
		n += count(c, predicate)
	return n

# Note: Terrain3D only loads its regions once the tree runs, and reading its surface before that crashes Godot.
func _initialize() -> void:
	_build_all.call_deferred()

func _build_all() -> void:
	var failures := 0
	var only := OS.get_cmdline_user_args()
	for name in MAPS:
		if not only.is_empty() and not only.has(name):
			continue
		var terrain3d := TERRAIN3D_ASSETS.has(name)
		if terrain3d and not ClassDB.class_exists(&"Terrain3D"):
			print("%s: skipped, Terrain3D is not installed (python3 tools/fetch_terrain3d.py)" % name)
			continue
		var started := Time.get_ticks_msec()
		var map_file := "res://demo/maps/showcase/%s.tdm" % name
		var root := Node3D.new()
		root.name = name.to_pascal_case()

		var map := TrenchDotMap.new()
		map.name = "Map"
		map.map_settings = load(SETTINGS)
		map.local_map_file = map_file
		root.add_child(map)
		map.owner = root
		var overlay_file := "res://demo/overlays/%s_overlay.tscn" % name
		var overlay: TrenchDotOverlay = null
		if ResourceLoader.exists(overlay_file):
			overlay = (load(overlay_file) as PackedScene).instantiate() as TrenchDotOverlay
			map.add_child(overlay)
			overlay.owner = root
		if terrain3d:
			# Terrain3D loads its regions when it enters the tree, and the scatter step reads its surface.
			get_root().add_child(root)
		map.build()
		reown(map, map, root)
		if terrain3d:
			for slot in map.find_children("*", "TrenchDotTerrain3D", true, false):
				# Note: Terrain3D reloads an assets resource that has a path when the scene enters the tree, and the
				# copy comes back without textures, so the scene embeds a pathless copy. Without a file the
				# editor texture cleanup would warn on every load, so it is off.
				slot.get_terrain().set("free_editor_textures", false)
				slot.get_terrain().set("assets", load(TERRAIN3D_ASSETS[name]).duplicate())
		if overlay:
			print("%s: overlay %s, sidecar %s" % [name, overlay_file, overlay.write_sidecar()])

		var meshes := count(map, func(n): return n is MeshInstance3D)
		var terrains := count(map, func(n): return n is TrenchDotTerrain)
		var slots := count(map, func(n): return n is TrenchDotTerrain3D and n.get_terrain() != null)
		var scatters := count(map, func(n): return n is TrenchDotScatter)
		var instances := count(map, func(n): return n is MultiMeshInstance3D)
		var lights := count(map, func(n): return n is Light3D)
		var environments := count(map, func(n): return n is WorldEnvironment)
		if root.is_inside_tree():
			get_root().remove_child(root)
		var packed := PackedScene.new()
		var err := packed.pack(root)
		if err == OK:
			err = ResourceSaver.save(packed, "res://demo/showcase/%s.scn" % name)
		print("%s: %d mesh instances, %d terrains, %d Terrain3D slots, %d scatter sets (%d multimeshes), %d lights, built in %.1f s, save %s" % [name, meshes, terrains, slots, scatters, instances, lights, (Time.get_ticks_msec() - started) / 1000.0, error_string(err)])
		var ground_ok := (terrains == 0 and slots == 1) if terrain3d else (terrains == 1 and slots == 0)
		if err != OK or not ground_ok or scatters < 2 or instances < 4 or meshes < (3 if terrain3d else 10) or environments != 1:
			failures += 1
		root.free()
	quit(1 if failures > 0 else 0)
