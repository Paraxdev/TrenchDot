extends SceneTree
## Builds a map whose scatter follows its Terrain3D before that Terrain3D has been ready in the tree, which crashed
## Godot inside Terrain3D. Prints the scattered origins and the Terrain3D warnings as JSON, see
## res://tests/terrain3d_scatter_tests.gd.

const Link := preload("res://tests/terrain3d_tests.gd")
const Scatter := preload("res://tests/terrain3d_scatter_tests.gd")

func _initialize() -> void:
	var settings: FuncGodotMapSettings = load(Link.SETTINGS)
	var json: Dictionary = JSON.parse_string(Link._map_text([Link.DATA]))
	var raw := [0, int(Link.VERTEX.x * settings.inverse_scale_factor), 40, int(Link.VERTEX.y * settings.inverse_scale_factor), 0, 0, 0, 1.0]
	json["layers"][0]["children"].append(Scatter._scatter(101, [Link.LINK_ID], [raw]))
	var level := Node3D.new()
	root.add_child(level)
	var map := FuncGodotMap.new()
	map.name = "Map"
	map.map_settings = settings
	map.local_map_file = Link.MAP
	level.add_child(map)
	map.owner = level
	var slot := TrenchDotTerrain3D.new()
	slot.name = TrenchDotTerrain3D.NODE_NAME
	map.add_child(slot)
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"data_directory", Link.DATA)
	slot.add_child(terrain)
	var report := map.build_from_text(JSON.stringify(json))
	var origins := Scatter._origins(Scatter._scatter_node(map, 101)).map(func(o: Vector3) -> Array: return [o.x, o.y, o.z])
	print(JSON.stringify({ "origins": origins, "warnings": Link._terrain3d_warnings(report) }))
	level.free()
	quit(0)
