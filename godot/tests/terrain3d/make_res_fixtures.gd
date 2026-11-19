## Writes the binary resources the Rust reader in td_formats/src/godot_res.rs is tested against. Run it by hand after a
## Godot update and commit what changed:
## godot --headless --path godot --script res://tests/terrain3d/make_res_fixtures.gd
extends SceneTree

const OUT := "res://tests/terrain3d/res_fixtures"


func _initialize() -> void:
	DirAccess.make_dir_recursive_absolute(OUT)
	var failed := _values()
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
