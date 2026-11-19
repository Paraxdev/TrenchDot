## Writes the binary resources the Rust readers in td_formats/src/godot_res.rs and terrain3d_res.rs are tested against,
## with the [code].td3r[/code] the addon exports for each region. Run it by hand after a Godot or Terrain3D update and
## commit what changed:
## godot --headless --path godot --script res://tests/terrain3d/make_res_fixtures.gd
extends SceneTree

const OUT := "res://tests/terrain3d/res_fixtures"


func _initialize() -> void:
	DirAccess.make_dir_recursive_absolute(OUT)
	var failed := _values()
	failed = _regions() or failed
	quit(1 if failed else 0)


## One metadata entry per value type Godot 4 saves, in a plain and a compressed file.
func _values() -> bool:
	var outside := Gradient.new()
	outside.offsets = PackedFloat32Array([0.0, 1.0])
	var failed := ResourceSaver.save(outside, OUT.path_join("outside.tres")) != OK
	var inside := Gradient.new()
	inside.offsets = PackedFloat32Array([0.25, 0.75])
	var transforms: Array[Transform3D] = [Transform3D(Basis.from_scale(Vector3(2, 3, 4)), Vector3(-1, 2, -3))]
	var cells: Dictionary[Vector2i, int] = {Vector2i(1, -2): 5}
	var values := {
		"nil": [null],
		"bool": true,
		"int": -7,
		"int64": 1 << 40,
		"float": 1.5,
		"double": 0.1,
		"string": "Grüße",
		"string_name": &"sn",
		"node_path": NodePath("/root/a:b:c"),
		"vector2": Vector2(1, -2),
		"vector2i": Vector2i(3, -4),
		"rect2": Rect2(1, 2, 3, 4),
		"rect2i": Rect2i(-1, -2, 3, 4),
		"vector3": Vector3(1, 2, -3),
		"vector3i": Vector3i(1, 2, -3),
		"vector4": Vector4(1, 2, 3, -4),
		"vector4i": Vector4i(1, 2, 3, -4),
		"plane": Plane(Vector3(0, 1, 0), 2),
		"quaternion": Quaternion(0, 0, 0, 1),
		"aabb": AABB(Vector3(1, 2, 3), Vector3(4, 5, 6)),
		"basis": Basis.from_scale(Vector3(2, 3, 4)),
		"transform2d": Transform2D(0, Vector2(5, 6)),
		"transform3d": transforms[0],
		"projection": Projection.IDENTITY,
		"color": Color(0.25, 0.5, 0.75, 1),
		"dictionary": {"k": [1, "two"], 3: null},
		"array": [1, [2, [3]]],
		"typed_array": transforms,
		"typed_dictionary": cells,
		"empty_array": [],
		"bytes": PackedByteArray([1, 2, 3]),
		"int32s": PackedInt32Array([1, -2]),
		"int64s": PackedInt64Array([1 << 40, -1]),
		"float32s": PackedFloat32Array([1.5, -0.25]),
		"float64s": PackedFloat64Array([0.1]),
		"strings": PackedStringArray(["a", "", "bc"]),
		"vector2s": PackedVector2Array([Vector2(1, 2)]),
		"vector3s": PackedVector3Array([Vector3(1, 2, 3), Vector3(-1, -2, -3)]),
		"colors": PackedColorArray([Color(1, 0, 0, 0.5)]),
		"vector4s": PackedVector4Array([Vector4(1, 2, 3, 4)]),
		"inside": inside,
		"outside": load(OUT.path_join("outside.tres")),
	}
	var resource := Resource.new()
	for key in values:
		resource.set_meta(key, values[key])
	failed = ResourceSaver.save(resource, OUT.path_join("values.res")) != OK or failed
	failed = ResourceSaver.save(resource, OUT.path_join("values_compressed.res"), ResourceSaver.FLAG_COMPRESS) != OK or failed
	return failed


## Regions of 64 vertices: 16 bit heights with painted color away from the origin, and instances with a blank color
## map. Note: headless Godot cannot compress a color map, the Rust tests build that case from these files.
func _regions() -> bool:
	if not ClassDB.class_exists(&"Terrain3DRegion"):
		push_error("Terrain3D is not installed in this project")
		return true
	var instances := { 0: { Vector2i(0, 0): [[Transform3D(Basis.IDENTITY, Vector3(1, 2, 3))], PackedColorArray([Color.WHITE]), true] } }
	var failed := _region("painted", Vector2i(-3, 2), 2.5, true, true, {})
	return _region("instances", Vector2i(1, 0), 1.0, false, false, instances) or failed


func _region(folder: String, loc: Vector2i, spacing: float, half: bool, painted: bool, instances: Dictionary) -> bool:
	const SIZE := 64
	var heights := PackedByteArray()
	var control := PackedByteArray()
	var color := PackedByteArray()
	heights.resize(SIZE * SIZE * 4)
	control.resize(SIZE * SIZE * 4)
	color.resize(SIZE * SIZE * 4)
	for y in SIZE:
		for x in SIZE:
			var i := (y * SIZE + x) * 4
			heights.encode_float(i, sin(x * 0.3) * 12.34 + y * 0.517 - 3.0)
			control.encode_u32(i, (x % 32) << 27 | (y % 32) << 22 | ((x * 7 + y) % 256) << 14 | (4 if (x + y) % 9 == 0 else 0) | x % 2)
			if painted:
				color.encode_u32(i, (x * 4) | (y * 4) << 8 | 128 << 16 | ((x + y) % 256) << 24)
	var region: Resource = ClassDB.instantiate(&"Terrain3DRegion")
	region.set(&"region_size", SIZE)
	region.set(&"vertex_spacing", spacing)
	region.set(&"location", loc)
	region.call(&"set_map", 0, Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, heights))
	region.call(&"set_map", 1, Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, control))
	var color_map := Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RGBA8, color)
	if not painted:
		color_map.fill(Color(1, 1, 1, 0.5))
	region.call(&"set_map", 2, color_map)
	if not instances.is_empty():
		region.set(&"instances", instances)
	var dir := OUT.path_join(folder)
	DirAccess.make_dir_recursive_absolute(dir)
	var path := dir.path_join(TrenchDotTerrain3DCache.region_stem(loc) + ".res")
	if region.call(&"save", path, half) != OK:
		push_error("could not save %s" % path)
		return true
	var saved := ResourceLoader.load(path, "", ResourceLoader.CACHE_MODE_IGNORE)
	var encoded := TrenchDotTerrain3DCache.encode_region(saved, loc, float(saved.get(&"vertex_spacing")))
	if encoded.has("problem"):
		push_error("%s: %s" % [path, encoded["problem"]])
		return true
	var file := FileAccess.open(path.get_basename() + TrenchDotTerrain3DCache.REGION_EXT, FileAccess.WRITE)
	file.store_buffer(encoded["bytes"])
	return false
