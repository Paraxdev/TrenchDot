extends RefCounted
## The Terrain3D cache exporter: what Terrain3D holds in memory lands in <data folder>/.trenchdot/ exactly as
## crates/td_formats reads it, byte for byte equal to the fixtures make_fixtures.py writes from the same patterns.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const WORK := "user://td_t3d_export"
const FIXTURES := "../crates/td_formats/tests/terrain3d/cache"
const TYPE_HEIGHT := 0
const TYPE_CONTROL := 1
const TYPE_COLOR := 2
const HOLE_BIT := 4

static func run(t) -> void:
	print("- Terrain3D cache export")
	if not ClassDB.class_exists("Terrain3D"):
		return
	_clear(WORK)
	await _test_fixture_bytes(t)
	await _test_layouts(t)
	await _test_large_region(t)
	await _test_incremental(t)
	await _test_data_versions(t)
	await _test_triggers(t)
	await _test_build_exports_in_editor_only(t)
	_test_cli(t)
	_clear(WORK)

static func _clear(dir: String) -> void:
	if not DirAccess.dir_exists_absolute(dir):
		return
	for sub in DirAccess.get_directories_at(dir):
		_clear(dir.path_join(sub))
	for file in DirAccess.get_files_at(dir):
		DirAccess.remove_absolute(dir.path_join(file))
	DirAccess.remove_absolute(dir)

## A Terrain3D in the tree on an empty folder, ready for its data to be filled.
static func _terrain(t, folder: String, region_size: int, spacing: float) -> Node3D:
	_clear(folder)
	DirAccess.make_dir_recursive_absolute(folder)
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"vertex_spacing", spacing)
	terrain.set(&"data_directory", folder)
	t.root.add_child(terrain)
	await t.process_frame
	# Note: Terrain3D ignores a region size set before its data exists.
	terrain.set(&"region_size", region_size)
	return terrain

## The maps make_fixtures.py writes for [param loc], as [code]height[/code], [code]control[/code] and
## [code]color[/code] images, color null for a region without one.
static func _pattern(loc: Vector2i, color: bool) -> Dictionary:
	const SIZE := 64
	var heights := PackedByteArray()
	var control := PackedByteArray()
	var rgba := PackedByteArray()
	heights.resize(SIZE * SIZE * 4)
	control.resize(SIZE * SIZE * 4)
	rgba.resize(SIZE * SIZE * 4 if color else 0)
	for z in SIZE:
		for x in SIZE:
			var i := z * SIZE + x
			heights.encode_float(i * 4, loc.x * 8 + loc.y * 0.5 + x * 0.25 - z * 0.125)
			var c := (i * 2654435761 + posmod(loc.x, 1 << 32) * 40503 + posmod(loc.y, 1 << 32) * 977) % (1 << 32)
			control.encode_u32(i * 4, { 0: 0x7FC00001, 1: 0xFF800001 }.get(i, c))
			if color:
				rgba.encode_u32(i * 4, (x * 4) | (z * 4) << 8 | ((x + z) % 256) << 16 | (255 - x) << 24)
	return {
		"height": Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, heights),
		"control": Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, control),
		"color": Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RGBA8, rgba) if color else null,
	}

static func _fill(terrain: Node3D, loc: Vector2i, maps: Dictionary) -> Object:
	var data: Object = terrain.get(&"data")
	data.add_region_blank(loc, false)
	var region: Object = data.get_region(loc)
	region.set_map(TYPE_HEIGHT, maps["height"])
	region.set_map(TYPE_CONTROL, maps["control"])
	if maps.get("color"):
		region.set_map(TYPE_COLOR, maps["color"])
	region.calc_height_range()
	return region

static func _fixture(path: String) -> PackedByteArray:
	return FileAccess.get_file_as_bytes(ProjectSettings.globalize_path("res://").path_join(FIXTURES).path_join(path).simplify_path())

## A .td3r file as [code]loc[/code], [code]size[/code], [code]spacing[/code], [code]color[/code] flag,
## [code]hash[/code], [code]payload[/code] and whether the hash is the sha256 of the payload.
static func _decode(bytes: PackedByteArray) -> Dictionary:
	if bytes.size() < 60 or bytes.slice(0, 4).get_string_from_ascii() != "TD3R":
		return {}
	var size := bytes.decode_u32(16)
	var color := bytes.decode_u32(24) & 1
	var payload := bytes.slice(60).decompress(size * size * (12 if color else 8), FileAccess.COMPRESSION_ZSTD)
	var hashing := HashingContext.new()
	hashing.start(HashingContext.HASH_SHA256)
	hashing.update(payload)
	return {
		"version": bytes.decode_u32(4), "loc": Vector2i(bytes.decode_s32(8), bytes.decode_s32(12)), "size": size,
		"spacing": bytes.decode_float(20), "color": color == 1, "hash": bytes.slice(28, 60).hex_encode(),
		"payload": payload, "hash_ok": hashing.finish() == bytes.slice(28, 60),
	}

## [param value] with every float made an int, the way JSON numbers compare to locations.
static func _ints(value: Variant) -> Variant:
	if value is Array:
		return value.map(_ints)
	return int(value) if value is float else value

static func _manifest(folder: String) -> Dictionary:
	return TrenchDotTerrain3DCache.read_manifest(folder)

static func _test_fixture_bytes(t) -> void:
	var folder := WORK.path_join("fixture")
	var terrain := await _terrain(t, folder, 64, 2.5)
	_fill(terrain, Vector2i(-1, 0), _pattern(Vector2i(-1, 0), true))
	_fill(terrain, Vector2i(3, -2), _pattern(Vector2i(3, -2), false))
	var result := TrenchDotTerrain3DCache.export(terrain, folder, null, { "editor_open": true, "map_transform_identity": false })
	t.check(result.ok and result.problems.is_empty(), "the fixture regions export, got %s" % [result])
	t.check(result.written == [Vector2i(3, -2), Vector2i(-1, 0)], "both regions are written, south first, got %s" % [result.written])
	for name in ["terrain3d-01_00.td3r", "terrain3d_03-02.td3r"]:
		var exported := FileAccess.get_file_as_bytes(folder.path_join(".trenchdot/regions").path_join(name))
		var fixture := _fixture("regions/" + name)
		t.check(not fixture.is_empty() and exported == fixture, "the exported %s equals the Rust fixture byte for byte (%d and %d bytes, header equal %s)" % [name, exported.size(), fixture.size(), exported.slice(0, 60) == fixture.slice(0, 60)])
	var m := _manifest(folder)
	var want: Dictionary = JSON.parse_string(_fixture("manifest.json").get_string_from_utf8())
	for key in ["format", "version", "terrain3d", "region_size", "vertex_spacing", "map_transform_identity", "editor_open"]:
		t.check(m.get(key) == want[key], "the manifest %s matches the fixture's, got %s, want %s" % [key, m.get(key), want[key]])
	var regions := (m.get("regions", []) as Array).map(func(r): return [r["loc"], r["hash"], r["height_range"]])
	var wanted := (want["regions"] as Array).map(func(r): return [r["loc"], r["hash"], r["height_range"]])
	wanted.reverse()
	t.check(regions == wanted, "the manifest regions match the fixture's, got %s, want %s" % [regions, wanted])
	t.check(m.get("godot") == "%d.%d.%d" % [Engine.get_version_info().major, Engine.get_version_info().minor, Engine.get_version_info().patch], "the manifest names the Godot version, got %s" % m.get("godot"))
	t.check(_ints(m.get("dirty")) == [[3, -2], [-1, 0]] and m.get("applied") == [], "regions Godot has not saved yet are dirty and nothing is applied, got %s %s" % [m.get("dirty"), m.get("applied")])
	t.check(not FileAccess.file_exists(folder.path_join(".trenchdot/manifest.json.tdpart")), "no .tdpart file stays behind")
	terrain.free()

## One region, sparse and negative layouts at spacing 2.5, with a hole and heights of plus and minus 20000 m.
static func _test_layouts(t) -> void:
	var folder := WORK.path_join("layouts")
	var terrain := await _terrain(t, folder, 64, 2.5)
	var data: Object = terrain.get(&"data")
	data.add_region_blank(Vector2i(-16, -16), false)
	var only := TrenchDotTerrain3DCache.export(terrain, folder)
	var single := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-16, -16))))
	t.check(only.ok and only.written == [Vector2i(-16, -16)], "one region exports, got %s" % [only])
	t.check(single.get("loc") == Vector2i(-16, -16) and single.get("size") == 64 and single.get("spacing") == 2.5 and single.get("version") == 1, "its header holds the location, size and spacing, got %s %s %s" % [single.get("loc"), single.get("size"), single.get("spacing")])
	t.check(single.get("hash_ok") and not single.get("color"), "its hash is the sha256 of its maps and a blank color map is left out, got %s %s" % [single.get("hash_ok"), single.get("color")])
	var blank := PackedFloat32Array()
	blank.resize(64 * 64)
	t.check(single.get("payload", PackedByteArray()).slice(0, 64 * 64 * 4) == blank.to_byte_array(), "a blank region exports flat at 0 m")

	data.add_region_blank(Vector2i(15, 15), false)
	data.add_region_blank(Vector2i(-1, 2), false)
	# World positions in meters: vertex (x, z) of region (15, 15) sits at ((15 * 64 + x) * 2.5, (15 * 64 + z) * 2.5).
	var high := Vector3((15 * 64 + 3) * 2.5, 0.0, (15 * 64 + 60) * 2.5)
	var low := Vector3((-1 * 64 + 63) * 2.5, 0.0, (2 * 64 + 0) * 2.5)
	data.set_height(high, 20000.0)
	data.set_height(low, -20000.0)
	data.set_control_hole(low, true)
	var sparse := TrenchDotTerrain3DCache.export(terrain, folder)
	t.check(sparse.ok and sparse.written == [Vector2i(-1, 2), Vector2i(15, 15)], "a sparse layout writes the two new regions only, got %s" % [sparse.written])
	var top := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(15, 15))))
	var bottom := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-1, 2))))
	t.check(top.get("payload", PackedByteArray()).decode_float((60 * 64 + 3) * 4) == 20000.0, "20000 m lands on its vertex")
	t.check(bottom.get("payload", PackedByteArray()).decode_float(63 * 4) == -20000.0, "-20000 m lands on its vertex")
	var hole: int = bottom.get("payload", PackedByteArray()).decode_u32(64 * 64 * 4 + 63 * 4)
	var beside: int = bottom.get("payload", PackedByteArray()).decode_u32(64 * 64 * 4 + 62 * 4)
	t.check(hole & HOLE_BIT and not beside & HOLE_BIT, "the hole bit is set on its vertex only, got %x and %x" % [hole, beside])
	var m := _manifest(folder)
	var locs := (m.get("regions", []) as Array).map(func(r): return r["loc"])
	t.check(_ints(locs) == [[-16, -16], [-1, 2], [15, 15]], "the manifest lists every region south first, got %s" % [locs])
	var ranges := (m.get("regions", []) as Array).map(func(r): return r["height_range"])
	t.check(ranges[1] == [-20000.0, 0.0] and ranges[2] == [0.0, 20000.0], "height ranges follow the edits, got %s" % [ranges])
	terrain.free()

static func _test_large_region(t) -> void:
	var folder := WORK.path_join("large")
	var terrain := await _terrain(t, folder, 2048, 1.0)
	var data: Object = terrain.get(&"data")
	data.add_region_blank(Vector2i(-1, 0), false)
	data.set_height(Vector3(-2048 + 2047, 0.0, 2047), 37.5)
	var result := TrenchDotTerrain3DCache.export(terrain, folder)
	var big := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-1, 0))))
	t.check(result.ok and big.get("size") == 2048 and big.get("hash_ok"), "a 2048 region exports, got %s %s" % [result, big.get("size")])
	t.check(big.get("payload", PackedByteArray()).decode_float((2047 * 2048 + 2047) * 4) == 37.5, "its last vertex keeps its height")
	t.check(int(_manifest(folder).get("region_size")) == 2048, "the manifest says 2048")
	terrain.free()

## Only changed regions are rewritten, a region list limits the export, and a removed region's file goes.
static func _test_incremental(t) -> void:
	var folder := WORK.path_join("incremental")
	var terrain := await _terrain(t, folder, 64, 1.0)
	var data: Object = terrain.get(&"data")
	var a := Vector2i(2, -3)
	var b := Vector2i(-4, 1)
	data.add_region_blank(a, false)
	data.add_region_blank(b, false)
	t.check(TrenchDotTerrain3DCache.export(terrain, folder).written.size() == 2, "the first export writes both regions")
	t.check(TrenchDotTerrain3DCache.export(terrain, folder).written == [], "an export without changes writes no region")
	var hash_b: String = _manifest(folder)["regions"][1]["hash"]
	data.set_height(Vector3(2 * 64 + 5.0, 0.0, -3 * 64 + 7.0), 4.0)
	t.check(TrenchDotTerrain3DCache.export(terrain, folder).written == [a], "a changed region is the only one written")
	t.check(_manifest(folder)["regions"][1]["hash"] == hash_b, "the other region keeps its hash")

	data.set_height(Vector3(-4 * 64 + 1.0, 0.0, 64 + 1.0), 9.0)
	data.set_height(Vector3(2 * 64 + 6.0, 0.0, -3 * 64 + 7.0), 5.0)
	var before_a: String = _manifest(folder)["regions"][0]["hash"]
	var listed := TrenchDotTerrain3DCache.export(terrain, folder, [b])
	t.check(listed.written == [b], "a region list writes only the listed region, got %s" % [listed.written])
	t.check(_manifest(folder)["regions"][0]["hash"] == before_a, "an unlisted region keeps its last exported hash")
	var none := TrenchDotTerrain3DCache.export(terrain, folder, [], { "editor_open": true })
	t.check(none.ok and none.written == [] and _manifest(folder).get("editor_open") == true, "an empty list only rewrites the manifest")

	data.remove_regionl(a, false)
	var removed := TrenchDotTerrain3DCache.export(terrain, folder)
	t.check(removed.removed == [TrenchDotTerrain3DCache.region_stem(a) + ".td3r"], "a removed region's file is deleted, got %s" % [removed.removed])
	t.check(not FileAccess.file_exists(TrenchDotTerrain3DCache.region_path(folder, a)) and FileAccess.file_exists(TrenchDotTerrain3DCache.region_path(folder, b)), "the other region's file stays")
	t.check((_manifest(folder)["regions"] as Array).map(func(r): return _ints(r["loc"])) == [[b.x, b.y]], "the manifest lists the remaining region")
	terrain.free()

## A region Terrain3D saved exports clean, older data exports as it is, newer data exports and is reported.
static func _test_data_versions(t) -> void:
	var saved := WORK.path_join("saved")
	DirAccess.make_dir_recursive_absolute(saved)
	DirAccess.copy_absolute("res://tests/terrain3d/probe/terrain3d-01_02.res", saved.path_join("terrain3d-01_02.res"))
	var loaded := ClassDB.instantiate(&"Terrain3D") as Node3D
	loaded.set(&"data_directory", saved)
	t.root.add_child(loaded)
	await t.process_frame
	var clean := TrenchDotTerrain3DCache.export(loaded, saved)
	var m := _manifest(saved)
	t.check(clean.ok and clean.written == [Vector2i(-1, 2)] and clean.problems.is_empty(), "a region Terrain3D saved exports, got %s" % [clean])
	t.check(m.get("dirty") == [] and is_equal_approx(float(m.get("data_version", 0.0)), 1.1), "it is not dirty and carries data version 1.10, got %s %s" % [m.get("dirty"), m.get("data_version")])
	t.check(_ints(m["regions"][0]["loc"]) == [-1, 2] and m["regions"][0]["height_range"] == [0.0, 12.5], "its height range holds the probe's 12.5 m, got %s" % [m["regions"]])
	loaded.free()

	# Saved by Terrain3D 1.0.2 at region size 64 and spacing 2: heights 0.5 x - 0.25 z + 3, a hole at pixel (10, 20)
	# and base texture 3 at (30, 5).
	var older := WORK.path_join("v1_0_2")
	DirAccess.make_dir_recursive_absolute(older)
	DirAccess.copy_absolute("res://tests/terrain3d/v1_0_2/terrain3d_02-01.res", older.path_join("terrain3d_02-01.res"))
	loaded = ClassDB.instantiate(&"Terrain3D") as Node3D
	loaded.set(&"vertex_spacing", 2.0)
	loaded.set(&"data_directory", older)
	t.root.add_child(loaded)
	await t.process_frame
	var exported := TrenchDotTerrain3DCache.export(loaded, older)
	m = _manifest(older)
	t.check(exported.ok and exported.written == [Vector2i(2, -1)] and exported.problems.is_empty(), "a region Terrain3D 1.0.2 saved exports, got %s" % [exported])
	t.check(m.get("region_size") == 64 and _ints(m.get("dirty")) == [[2, -1]] and is_equal_approx(float(m.get("data_version", 0.0)), 1.1), "it keeps region size 64, and Terrain3D upgrades its data version 0.93 to 1.10 on load, so it stays dirty until Godot saves it, got %s %s %s" % [m.get("region_size"), m.get("dirty"), m.get("data_version")])
	var file := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(older, Vector2i(2, -1))))
	var payload: PackedByteArray = file.get("payload", PackedByteArray())
	var at := func(x: int, z: int, offset: int) -> int: return offset + (z * 64 + x) * 4
	t.check(file.get("hash_ok", false) and file.get("size") == 64 and is_equal_approx(file.get("spacing", 0.0), 2.0) and not file.get("color", true), "its file holds a 64 vertex region at spacing 2 without color, got %s" % [[file.get("hash_ok"), file.get("size"), file.get("spacing"), file.get("color")]])
	t.check(payload.size() == 64 * 64 * 8 and is_equal_approx(payload.decode_float(at.call(5, 3, 0)), 4.75) and is_equal_approx(payload.decode_float(at.call(63, 63, 0)), 18.75), "its heights survive, got %s %s" % [payload.decode_float(at.call(5, 3, 0)), payload.decode_float(at.call(63, 63, 0))])
	var control := 64 * 64 * 4
	t.check(payload.decode_u32(at.call(10, 20, control)) & HOLE_BIT and not payload.decode_u32(at.call(11, 20, control)) & HOLE_BIT, "its hole stays on its vertex")
	t.check(payload.decode_u32(at.call(30, 5, control)) >> 27 == 3 and payload.decode_u32(at.call(31, 5, control)) >> 27 == 0, "its base texture stays on its vertex")
	loaded.free()

	var folder := WORK.path_join("versions")
	var terrain := await _terrain(t, folder, 64, 1.0)
	var data: Object = terrain.get(&"data")
	data.add_region_blank(Vector2i(0, 0), false)
	data.get_region(Vector2i(0, 0)).set(&"version", 1.0)
	var old := TrenchDotTerrain3DCache.export(terrain, folder)
	t.check(old.ok and old.problems.is_empty() and is_equal_approx(float(_manifest(folder)["data_version"]), 1.0), "data version 1.0 exports without a problem, got %s" % [old])
	data.get_region(Vector2i(0, 0)).set(&"version", 1.2)
	var newer := TrenchDotTerrain3DCache.export(terrain, folder)
	t.check(newer.ok and newer.problems.size() == 1 and newer.problems[0].contains("1.20") and newer.problems[0].contains("newer"), "data version 1.2 exports and is reported, got %s" % [newer.problems])
	t.check(is_equal_approx(float(_manifest(folder)["data_version"]), 1.2), "the manifest carries the newer version")
	terrain.free()

static func _wait(t, seconds: float) -> void:
	await t.create_timer(seconds).timeout

## A slot holding a Terrain3D on [param folder] with regions [param locs], like the build makes, outside a map.
static func _slot(t, folder: String, locs: Array) -> TrenchDotTerrain3D:
	var terrain := await _terrain(t, folder, 64, 2.0)
	terrain.get_parent().remove_child(terrain)
	var slot := TrenchDotTerrain3D.new()
	t.root.add_child(slot)
	slot.add_child(terrain)
	await t.process_frame
	for loc in locs:
		terrain.get(&"data").add_region_blank(loc, false)
	return slot

## Strokes export after half a second of quiet, once for the regions they touched, scene saves export what changed.
static func _test_triggers(t) -> void:
	TrenchDotTerrain3D.export_outside_editor = true
	var folder := WORK.path_join("triggers")
	var a := Vector2i(-2, 1)
	var b := Vector2i(1, -1)
	var c := Vector2i(3, 3)
	var slot := await _slot(t, folder, [a, b, c])
	var data: Object = slot.get_terrain().get(&"data")
	var exports: Array = []
	slot.cache_exported.connect(func(r): exports.append(r))
	slot.export_cache()
	exports.clear()
	# One region is 64 vertices of 2 m.
	var in_a := Vector3(-2 * 128 + 10.0, 0.0, 128 + 10.0)
	var in_b := Vector3(128 + 6.0, 0.0, -128 + 100.0)
	data.set_height(in_a, 3.0)
	data.set_height(in_b, -4.0)
	data.set_height(Vector3(3 * 128 + 1.0, 0.0, 3 * 128 + 1.0), 8.0)
	data.emit_signal(&"maps_edited", AABB(in_a - Vector3(4, 0, 4), Vector3(8, 3, 8)))
	await _wait(t, 0.3)
	data.emit_signal(&"maps_edited", AABB(in_b, Vector3(1, 1, 1)))
	await _wait(t, 0.35)
	t.check(exports.is_empty(), "a stroke within half a second of the last waits, got %d exports" % exports.size())
	await _wait(t, 0.35)
	t.check(exports.size() == 1, "two strokes export once, got %d" % exports.size())
	t.check(exports.size() == 1 and exports[0].written == [b, a], "the export writes the two stroked regions only, got %s" % [exports[0].written if exports else []])

	var corner := AABB(Vector3(-0.5, 0.0, -0.5), Vector3(1.0, 1.0, 1.0))
	data.add_region_blank(Vector2i(0, 0), false)
	data.add_region_blank(Vector2i(-1, -1), false)
	exports.clear()
	data.emit_signal(&"maps_edited", corner)
	await _wait(t, 0.7)
	t.check(exports.size() == 1 and exports[0].written == [Vector2i(-1, -1), Vector2i(0, 0)], "a stroke over a region corner exports the regions it touches that exist, got %s" % [exports[0].written if exports else []])

	exports.clear()
	slot.notification(Node.NOTIFICATION_EDITOR_PRE_SAVE)
	t.check(exports.size() == 1 and exports[0].written == [c], "saving the scene exports the region the strokes missed, got %s" % [exports[0].written if exports else []])
	t.check(exports.size() == 1 and _manifest(folder).get("textures") == [], "saving refreshes the texture list")
	t.check(_ints(_manifest(folder).get("dirty")).size() == 5, "unsaved regions are dirty before the save")
	for loc in data.get_region_locations():
		data.get_region(loc).set(&"modified", false)
	exports.clear()
	slot.notification(Node.NOTIFICATION_EDITOR_POST_SAVE)
	t.check(exports.size() == 1 and exports[0].written == [] and _manifest(folder).get("dirty") == [], "after the save nothing is dirty and no region is rewritten, got %s" % [exports])
	slot.free()
	TrenchDotTerrain3D.export_outside_editor = false

## Building a map writes the cache in the editor, never in a game or a headless tool.
static func _test_build_exports_in_editor_only(t) -> void:
	for editor in [false, true]:
		TrenchDotTerrain3D.export_outside_editor = editor
		var folder := WORK.path_join("build_%s" % editor)
		_clear(folder)
		DirAccess.make_dir_recursive_absolute(folder)
		var map := Node3D.new()
		t.root.add_child(map)
		var report := TrenchDotBuildReport.new("res://tests/maps/basic.tdm")
		var links: Array[Dictionary] = [{ "data": { "data_directory": folder } }]
		var slot := TrenchDotTerrain3D.build(map, links, report)
		await t.process_frame
		slot.get_terrain().get(&"data").add_region_blank(Vector2i(-3, 2), false)
		await _wait(t, 0.7)
		var m := _manifest(folder)
		if editor:
			t.check(_ints(m.get("regions", []).map(func(r): return r["loc"])) == [[-3, 2]], "a build in the editor exports the cache, got %s" % [m])
		else:
			t.check(not DirAccess.dir_exists_absolute(TrenchDotTerrain3DCache.cache_dir(folder)), "a build outside the editor writes no cache")
		map.free()
	TrenchDotTerrain3D.export_outside_editor = false

## Runs the export CLI in a headless Godot, returns its exit code and the JSON of its last line.
static func _cli(args: Array) -> Array:
	var output: Array = []
	var cli_args := ["--headless", "--path", ProjectSettings.globalize_path("res://"), "--script", "res://addons/func_godot/src/trenchdot/terrain3d_cli.gd", "--"]
	var code := OS.execute(OS.get_executable_path(), cli_args + args, output, true)
	var lines := "
".join(output).strip_edges().split("
")
	var parsed: Variant = JSON.parse_string(lines[lines.size() - 1])
	return [code, parsed if parsed is Dictionary else {}]

static func _test_cli(t) -> void:
	var folder := WORK.path_join("cli")
	_clear(folder)
	DirAccess.make_dir_recursive_absolute(folder)
	DirAccess.copy_absolute("res://tests/terrain3d/probe/terrain3d-01_02.res", folder.path_join("terrain3d-01_02.res"))
	var first := _cli(["export", folder])
	t.check(first[0] == 0 and first[1].get("ok") == true and _ints(first[1].get("written")) == [[-1, 2]], "the CLI exports a saved folder, got %s" % [first])
	var m := _manifest(folder)
	t.check(m.get("editor_open") == false and float(m.get("vertex_spacing", 0.0)) == 1.0 and FileAccess.file_exists(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-1, 2))), "it writes the manifest and the region file, got %s" % [m])

	m["vertex_spacing"] = 2.5
	FileAccess.open(folder.path_join(".trenchdot/manifest.json"), FileAccess.WRITE).store_string(JSON.stringify(m))
	var second := _cli(["export", folder])
	var header := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-1, 2))))
	t.check(second[0] == 0 and float(_manifest(folder).get("vertex_spacing", 0.0)) == 2.5 and header.get("spacing") == 2.5, "the CLI keeps the vertex spacing of the last manifest, got %s %s" % [second, header.get("spacing")])

	var spaced := WORK.path_join("cli_spaced")
	_clear(spaced)
	DirAccess.make_dir_recursive_absolute(spaced)
	DirAccess.copy_absolute("res://tests/terrain3d/v1_0_2/terrain3d_02-01.res", spaced.path_join("terrain3d_02-01.res"))
	var third := _cli(["export", spaced])
	header = _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(spaced, Vector2i(2, -1))))
	t.check(third[0] == 0 and float(_manifest(spaced).get("vertex_spacing", 0.0)) == 2.0 and header.get("spacing") == 2.0, "without a manifest the CLI takes the vertex spacing of the region files, got %s %s" % [_manifest(spaced).get("vertex_spacing"), header.get("spacing")])

	var missing := _cli(["export", WORK.path_join("nowhere")])
	t.check(missing[0] == 1 and missing[1].get("ok") == false and str(missing[1].get("error")).contains("nowhere"), "the CLI refuses a missing folder, got %s" % [missing])
	var usage := _cli(["apply"])
	t.check(usage[0] == 1 and str(usage[1].get("error")).contains("usage"), "the CLI refuses an unknown command, got %s" % [usage])
