extends RefCounted
## Samples of Terrain3D's own get_surface_height over three exported regions, the oracle
## crates/td_formats/tests/terrain3d_parity.rs checks td_geom::terrain3d::surface_height against.
##
## The committed folder holds the exported cache and samples.json, [code]none[/code] without a world background and
## [code]flat[/code] with the Flat background blending toward the ground level near open edges. Headless Godot has
## no shader parameters, so Terrain3D skips the blend there and only [code]none[/code] is checked. Run the suite in a
## window to check [code]flat[/code] too, and with TRENCHDOT_WRITE_PARITY=1 to rewrite the committed folder.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const WORK := "user://td_t3d_parity"
const FIXTURES := "../crates/td_formats/tests/terrain3d/parity"
const SPACING := 2.5
const SIZE := 64
const REGIONS: Array[Vector2i] = [Vector2i(-2, 1), Vector2i(-1, 1), Vector2i(-1, 2)]
const HOLES: Array[Vector2i] = [Vector2i(-100, 90), Vector2i(-30, 100), Vector2i(-1, 130), Vector2i(-64, 70)]
const GROUND_LEVEL := -3.0
const REGION_BLEND := 0.9
const BACKGROUND_FLAT := 1

static func run(t) -> void:
	print("- Terrain3D surface height parity")
	if not ClassDB.class_exists("Terrain3D"):
		return
	var folder := WORK
	_clear(folder)
	DirAccess.make_dir_recursive_absolute(folder)
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"vertex_spacing", SPACING)
	terrain.set(&"data_directory", folder)
	t.root.add_child(terrain)
	await t.process_frame
	# Note: Terrain3D ignores a region size set before its data exists.
	terrain.set(&"region_size", SIZE)
	var data: Object = terrain.get(&"data")
	for loc in REGIONS:
		data.add_region_blank(loc, true)
		var region: Object = data.get_region(loc)
		region.set_map(0, _heights(loc))
	for v in HOLES:
		data.set_control_hole(Vector3(v.x * SPACING, 0.0, v.y * SPACING), true)
	t.check(data.get_control_hole(Vector3(-30 * SPACING, 0.0, 100 * SPACING)) and not data.get_control_hole(Vector3(-31 * SPACING, 0.0, 100 * SPACING)), "the hole lands on its vertex only")

	var material: Object = terrain.get(&"material")
	material.set(&"world_background", 0)
	await t.process_frame
	var samples := { "none": _sample(data) }
	material.set(&"world_background", BACKGROUND_FLAT)
	await t.process_frame
	# Note: the shader only has these uniforms once the background needs them, until then setting them does nothing.
	material.set_shader_param(&"ground_level", GROUND_LEVEL)
	material.set_shader_param(&"region_blend", REGION_BLEND)
	var blends: bool = material.get_shader_param(&"region_blend") != null
	if blends:
		samples["flat"] = _sample(data)
	var result := TrenchDotTerrain3DCache.export(terrain, folder, null, { "editor_open": false })
	t.check(result.ok and result.written.size() == 3, "the parity regions export, got %s" % [result])
	terrain.free()

	var cache := folder.path_join(TrenchDotTerrain3DCache.DIR_NAME)
	var points: Array = samples["none"]
	t.check(points.size() == 200, "200 points are sampled, got %d" % points.size())
	t.check(points.any(func(p): return p[2] == null) and points.any(func(p): return p[2] != null), "the samples hold both heights and holes")
	if blends:
		var differ := range(points.size()).filter(func(i): return samples["flat"][i][2] != points[i][2])
		t.check(differ.size() >= 20, "the Flat background changes heights near open edges, %d samples differ" % differ.size())
	if OS.get_environment("TRENCHDOT_WRITE_PARITY") == "1":
		_write_fixtures(t, cache, samples, blends)
	_compare(t, cache, samples)
	_clear(folder)

## Slanted and wavy heights over the whole vertex grid, so neighbors across a seam continue the same surface.
static func _heights(loc: Vector2i) -> Image:
	var bytes := PackedByteArray()
	bytes.resize(SIZE * SIZE * 4)
	for z in SIZE:
		for x in SIZE:
			var vx := loc.x * SIZE + x
			var vz := loc.y * SIZE + z
			bytes.encode_float((z * SIZE + x) * 4, 0.37 * vx - 0.21 * vz + 4.0 * sin(vx * 0.3) * cos(vz * 0.17) + 12.0)
	return Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, bytes)

## Positions in meters on vertices, between them, by holes, outside every region, across seams and open edges, and
## either side of the 0.01 m radius inside which Terrain3D reads the vertex instead of interpolating. Every value is a
## multiple of 1/256 so it is exact in f32 and in JSON.
static func _points() -> Array:
	var out := []
	for k in 25:
		out.append(Vector2(-128 + (k * 53) % 128, 64 + (k * 29) % 128) * SPACING)
	var state := 1234567
	for k in 60:
		state = (state * 1103515245 + 12345) % 2147483648
		var x := -330.0 + (state % 87040) / 256.0
		state = (state * 1103515245 + 12345) % 2147483648
		out.append(Vector2(x, 150.0 + (state % 87040) / 256.0))
	for v in HOLES:
		for offset in [Vector2.ZERO, Vector2(0.6, 0.3), Vector2(-0.4, 1.1), Vector2(2.4, -0.2), Vector2(-2.7, -1.3)]:
			out.append(Vector2(v) * SPACING + offset)
	for p in [Vector2(-500, 200), Vector2(100, 300), Vector2(-200, 600), Vector2(-200, -10), Vector2(-240, 400),
			Vector2(-161, 330), Vector2(0.6, 200), Vector2(-321, 250), Vector2(-150, 159), Vector2(-50, 481),
			Vector2(-330, 330), Vector2(5000, -5000), Vector2(-5000, 5000), Vector2(-80, 159.99), Vector2(-1.2, 485)]:
		out.append(p)
	for d in [-0.75, -0.01, 0.003, 0.4, 1.3, 2.2]:
		for along in [0, 1]:
			out.append(Vector2(-160.0 + d, [201.3, 287.9][along]))
			out.append(Vector2([-120.7, -33.1][along], 320.0 + d))
			out.append(Vector2(-3.0 + d, [350.2, 455.6][along]))
			out.append(Vector2([-300.4, -201.9][along], 315.0 + d))
			out.append(Vector2([-317.3, -250.6][along], 160.0 + d))
	for v in [Vector2i(-110, 80), Vector2i(-64, 101), Vector2i(-30, 100), Vector2i(-1, 150), Vector2i(-20, 128)]:
		for offset in [Vector2(2, 0), Vector2(-2, 1), Vector2(-3, 0), Vector2(2, 2)]:
			out.append(Vector2(v) * SPACING + offset / 256.0)
	return out.map(func(p: Vector2): return (p * 256.0).round() / 256.0)

static func _sample(data: Object) -> Array:
	var out := []
	for p: Vector2 in _points():
		var h: float = data.get_surface_height(Vector3(p.x, 0.0, p.y))
		out.append([p.x, p.y, null if is_nan(h) else h])
	return out

static func _fixture(path: String) -> String:
	return ProjectSettings.globalize_path("res://").path_join(FIXTURES).path_join(path).simplify_path()

static func _write_fixtures(t, cache: String, samples: Dictionary, blends: bool) -> void:
	t.check(blends, "the parity fixtures are written from a windowed run, where the Flat background blends")
	if not blends:
		return
	DirAccess.make_dir_recursive_absolute(_fixture("regions"))
	DirAccess.copy_absolute(cache.path_join(TrenchDotTerrain3DCache.MANIFEST), _fixture(TrenchDotTerrain3DCache.MANIFEST))
	for name in DirAccess.get_files_at(cache.path_join("regions")):
		DirAccess.copy_absolute(cache.path_join("regions").path_join(name), _fixture("regions").path_join(name))
	var file := FileAccess.open(_fixture("samples.json"), FileAccess.WRITE)
	file.store_string(JSON.stringify(samples, "", true, true) + "\n")
	file.close()

static func _compare(t, cache: String, samples: Dictionary) -> void:
	for name in DirAccess.get_files_at(cache.path_join("regions")):
		var exported := FileAccess.get_file_as_bytes(cache.path_join("regions").path_join(name))
		var committed := FileAccess.get_file_as_bytes(_fixture("regions").path_join(name))
		t.check(not committed.is_empty() and exported == committed, "the exported %s equals the committed parity region" % name)
	var m := TrenchDotTerrain3DCache.read_manifest(cache.get_base_dir())
	var want: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(_fixture(TrenchDotTerrain3DCache.MANIFEST)))
	for key in ["region_size", "vertex_spacing", "regions"]:
		t.check(m.get(key) == want.get(key), "the parity manifest %s matches the committed one, got %s, want %s" % [key, m.get(key), want.get(key)])
	t.check(want["material"].get("world_background") == BACKGROUND_FLAT and is_equal_approx(want["material"].get("ground_level", 0.0), GROUND_LEVEL) and is_equal_approx(want["material"].get("region_blend", 0.0), REGION_BLEND), "the committed manifest records the Flat background, got %s" % [want["material"]])
	var committed: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(_fixture("samples.json")))
	for mode in samples:
		var got: Array = samples[mode]
		var expected: Array = committed.get(mode, [])
		var wrong := range(got.size()).filter(func(i): return i >= expected.size() or not _same(got[i], expected[i]))
		t.check(got.size() == expected.size() and wrong.is_empty(), "the %s samples match the committed ones, first differing %s" % [mode, wrong.slice(0, 5).map(func(i): return [got[i], expected[i] if i < expected.size() else null])])

static func _same(a: Array, b: Array) -> bool:
	if a[0] != b[0] or a[1] != b[1] or (a[2] == null) != (b[2] == null):
		return false
	return a[2] == null or absf(a[2] - b[2]) <= 1e-6

static func _clear(dir: String) -> void:
	if not DirAccess.dir_exists_absolute(dir):
		return
	for sub in DirAccess.get_directories_at(dir):
		_clear(dir.path_join(sub))
	for file in DirAccess.get_files_at(dir):
		DirAccess.remove_absolute(dir.path_join(file))
	DirAccess.remove_absolute(dir)
