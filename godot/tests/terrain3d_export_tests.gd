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
	await _test_textures(t)
	await _test_texture_looks(t)
	await _test_material(t)
	await _test_ocean(t)
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
	t.check(terrain.get(&"data").get_region_locations() == [Vector2i(-1, 0), Vector2i(3, -2)], "Terrain3D keeps its region list in the order it had, got %s" % [terrain.get(&"data").get_region_locations()])
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

## A texture with a block compressed albedo like an imported one gets a thumbnail of it. Terrain3D fills a texture
## without an albedo with the checker it draws, so its thumbnail is that checker.
static func _test_textures(t) -> void:
	var folder := WORK.path_join("textures")
	var terrain := await _terrain(t, folder, 64, 1.0)
	terrain.get(&"data").add_region_blank(Vector2i(1, -2), false)
	var rock: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
	rock.set(&"name", "Rock")
	rock.set(&"albedo_color", Color(0.5, 0.25, 0.125, 1.0))
	var assets: Resource = ClassDB.instantiate(&"Terrain3DAssets")
	assets.call(&"set_texture_asset", 0, rock)
	terrain.set(&"assets", assets)
	await t.process_frame
	var cache := TrenchDotTerrain3DCache.cache_dir(folder)
	var first := TrenchDotTerrain3DCache.export(terrain, folder)
	var textures: Array = _manifest(folder).get("textures", [])
	t.check(first.ok and textures.size() == 1 and _ints(textures[0]["id"]) == 0 and textures[0]["name"] == "Rock" and textures[0]["albedo_color"] == [0.5, 0.25, 0.125, 1.0] and textures[0]["thumbnail"] == "textures/0.png", "a texture without an albedo is listed with its color and a thumbnail, got %s" % [textures])
	var checker := Image.load_from_file(cache.path_join("textures/0.png"))
	var corners := [Vector2i(0, 0), Vector2i(63, 0), Vector2i(0, 63), Vector2i(63, 63), Vector2i(31, 31)].map(func(p): return checker.get_pixelv(p) if checker else null)
	t.check(checker and checker.get_size() == Vector2i(64, 64) and corners.any(func(c): return c != corners[0]), "its thumbnail is the 64 pixel checker, got %s" % [corners])

	var image := Image.create_empty(128, 128, false, Image.FORMAT_RGBA8)
	image.fill_rect(Rect2i(0, 0, 64, 128), Color8(200, 40, 30))
	image.fill_rect(Rect2i(64, 0, 64, 128), Color8(20, 90, 220))
	image.generate_mipmaps()
	image.compress(Image.COMPRESS_S3TC)
	var moss: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
	moss.set(&"name", "Moss")
	moss.set(&"albedo_color", Color(0.75, 1.0, 0.5, 1.0))
	moss.set(&"albedo_texture", ImageTexture.create_from_image(image))
	assets.call(&"set_texture_asset", 1, moss)
	await t.process_frame
	var second := TrenchDotTerrain3DCache.export(terrain, folder)
	textures = _manifest(folder).get("textures", [])
	t.check(second.ok and textures.size() == 2, "both textures are listed, got %s %s" % [second, textures])
	if textures.size() == 2:
		t.check(_ints(textures[1]["id"]) == 1 and textures[1]["name"] == "Moss" and textures[1]["albedo_color"] == [0.75, 1.0, 0.5, 1.0] and textures[1]["thumbnail"] == "textures/1.png", "the textured one has its color and a thumbnail, got %s" % [textures[1]])
		t.check(textures[0]["name"] == "Rock" and textures[0]["thumbnail"] == "textures/0.png", "the other keeps its checker, got %s" % [textures[0]])
	var thumbnail := Image.load_from_file(cache.path_join("textures/1.png"))
	t.check(thumbnail and thumbnail.get_size() == Vector2i(64, 64), "the thumbnail is 64 pixels square, got %s" % [thumbnail.get_size() if thumbnail else null])
	if thumbnail:
		var left := thumbnail.get_pixel(8, 40)
		var right := thumbnail.get_pixel(56, 20)
		t.check(left.r8 > 170 and left.b8 < 70 and right.b8 > 190 and right.r8 < 50, "the thumbnail shows the albedo halves, got %s %s" % [left, right])
	terrain.free()

## A 128 pixel square image, [param left] on its left half and [param right] on the other.
static func _halves(left: Color, right: Color) -> Image:
	var image := Image.create_empty(128, 128, false, Image.FORMAT_RGBA8)
	image.fill_rect(Rect2i(0, 0, 64, 128), left)
	image.fill_rect(Rect2i(64, 0, 64, 128), right)
	return image

static func _near_all(a: Variant, b: Array) -> bool:
	if not a is Array or a.size() != b.size():
		return false
	for i in b.size():
		if absf(float(a[i]) - float(b[i])) > 0.001:
			return false
	return true

## Whether [param image] shows [param left] at (100, 300) and [param right] at (400, 200), within a little.
static func _shows(image: Image, left: Color, right: Color) -> bool:
	if not image:
		return false
	var a := image.get_pixel(100, 300)
	var b := image.get_pixel(400, 200)
	return _near_all([a.r, a.g, a.b], [left.r, left.g, left.b]) and _near_all([b.r, b.g, b.b], [right.r, right.g, right.b])

## Each texture carries its uv values, its albedo at 512 pixels and a hash that follows the image and the uv values.
## An export reuses the images of a texture whose hash it already wrote.
static func _test_texture_looks(t) -> void:
	var folder := WORK.path_join("texture_looks")
	var terrain := await _terrain(t, folder, 64, 1.0)
	terrain.get(&"data").add_region_blank(Vector2i(-2, 3), false)
	var red := Color8(200, 40, 30)
	var blue := Color8(20, 90, 220)
	var moss := Color8(40, 160, 50)
	var lime := Color8(150, 210, 60)
	var cliff: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
	cliff.set(&"name", "Cliff")
	cliff.set(&"albedo_color", Color(0.75, 0.5, 0.25, 1.0))
	cliff.set(&"uv_scale", 0.25)
	cliff.set(&"detiling_rotation", 0.375)
	cliff.set(&"detiling_shift", 0.125)
	cliff.set(&"albedo_texture", ImageTexture.create_from_image(_halves(red, blue)))
	var grass: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
	grass.set(&"name", "Grass")
	grass.set(&"albedo_texture", ImageTexture.create_from_image(_halves(moss, lime)))
	var assets: Resource = ClassDB.instantiate(&"Terrain3DAssets")
	assets.call(&"set_texture_asset", 0, cliff)
	assets.call(&"set_texture_asset", 1, grass)
	terrain.set(&"assets", assets)
	await t.process_frame
	var cache := TrenchDotTerrain3DCache.cache_dir(folder)
	var first := TrenchDotTerrain3DCache.export(terrain, folder)
	var textures: Array = _manifest(folder).get("textures", [])
	t.check(first.ok and textures.size() == 2, "both textures are listed, got %s %s" % [first, textures])
	if textures.size() != 2:
		terrain.free()
		return
	var rock: Dictionary = textures[0]
	var lawn: Dictionary = textures[1]
	t.check(_ints(rock["id"]) == 0 and rock["name"] == "Cliff" and _near_all(rock["albedo_color"], [0.75, 0.5, 0.25, 1.0]) and _near_all([rock["uv_scale"], rock["detiling_rotation"], rock["detiling_shift"]], [0.25, 0.375, 0.125]) and rock["albedo"] == "textures/0_albedo.png" and rock["thumbnail"] == "textures/0.png", "the first texture has its tint, uv values and albedo, got %s" % [rock])
	t.check(_ints(lawn["id"]) == 1 and _near_all(lawn["albedo_color"], [1.0, 1.0, 1.0, 1.0]) and _near_all([lawn["uv_scale"], lawn["detiling_rotation"], lawn["detiling_shift"]], [0.1, 0.0, 0.0]) and lawn["albedo"] == "textures/1_albedo.png", "the second keeps Terrain3D's defaults, got %s" % [lawn])
	var hashes := [str(rock.get("hash", "")), str(lawn.get("hash", ""))]
	t.check(hashes.all(func(h: String): return h.length() == 64 and h.is_valid_hex_number()) and hashes[0] != hashes[1], "two textures have two hashes, got %s" % [hashes])
	var albedo := Image.load_from_file(cache.path_join("textures/0_albedo.png"))
	t.check(albedo and albedo.get_size() == Vector2i(512, 512) and albedo.get_format() == Image.FORMAT_RGB8, "the albedo is a 512 pixel RGB image, got %s %s" % [albedo.get_size() if albedo else null, albedo.get_format() if albedo else null])
	t.check(_shows(albedo, red, blue), "the albedo shows the image's halves")

	var marker := Image.create_empty(8, 8, false, Image.FORMAT_RGB8)
	FileAccess.open(cache.path_join("textures/1_albedo.png"), FileAccess.WRITE).store_buffer(marker.save_png_to_buffer())
	cliff.set(&"albedo_texture", ImageTexture.create_from_image(_halves(blue, red)))
	TrenchDotTerrain3DCache.export(terrain, folder)
	textures = _manifest(folder).get("textures", [])
	t.check(textures.size() == 2 and textures[0]["hash"] != hashes[0] and textures[1]["hash"] == hashes[1], "a new image changes only its own texture's hash, got %s" % [textures])
	t.check(_shows(Image.load_from_file(cache.path_join("textures/0_albedo.png")), blue, red), "the albedo shows the new image")
	var kept := Image.load_from_file(cache.path_join("textures/1_albedo.png"))
	t.check(kept and kept.get_size() == Vector2i(8, 8), "the unchanged texture's albedo is not written again, got %s" % [kept.get_size() if kept else null])

	var swapped: String = textures[0]["hash"]
	cliff.set(&"uv_scale", 0.5)
	DirAccess.remove_absolute(cache.path_join("textures/1_albedo.png"))
	TrenchDotTerrain3DCache.export(terrain, folder)
	textures = _manifest(folder).get("textures", [])
	t.check(textures.size() == 2 and textures[0]["hash"] != swapped and t.near(textures[0]["uv_scale"], 0.5), "a new uv scale changes the hash, got %s" % [textures])
	var rewritten := Image.load_from_file(cache.path_join("textures/1_albedo.png"))
	t.check(textures.size() == 2 and textures[1]["hash"] == hashes[1] and textures[1]["albedo"] == "textures/1_albedo.png" and _shows(rewritten, moss, lime), "a missing albedo is written again under the same hash, got %s" % [textures])

	cliff.set(&"name", "Cliff Face")
	TrenchDotTerrain3DCache.export(terrain, folder, null, { "scene_values_from_cache": true })
	textures = _manifest(folder).get("textures", [])
	t.check(textures.size() == 2 and textures[0]["name"] == "Cliff Face", "keeping the scene values of the cache still lists the textures of a node that has some, got %s" % [textures])
	terrain.free()

## The material's world background, noise, auto shader and blend values land in the manifest. Values the shader lacks,
## like the noise ones without a noise background, are the Terrain3D shader defaults.
static func _test_material(t) -> void:
	var folder := WORK.path_join("material")
	var terrain := await _terrain(t, folder, 64, 1.0)
	terrain.get(&"data").add_region_blank(Vector2i(2, -1), false)
	# Note: the headless renderer logs an error per mesh for a material swapped in early, so this edits Terrain3D's own.
	var material: Resource = terrain.get(&"material")
	material.set(&"world_background", 2)
	material.set(&"auto_shader_enabled", true)
	material.set(&"dual_scaling_enabled", true)
	material.set(&"macro_variation_enabled", true)
	material.set(&"projection_enabled", true)
	await t.process_frame
	var params := {
		&"world_noise_scale": 7.25, &"world_noise_height": -64.5, &"world_noise_offset": Vector3(12.5, -3.0, 0.75),
		&"world_noise_lod_distance": 9000.0, &"world_noise_max_octaves": 9, &"world_noise_min_octaves": 3,
		&"world_noise_fragment_normals": true, &"auto_slope": 2.5, &"auto_height_reduction": 0.375,
		&"auto_base_texture": 3, &"auto_overlay_texture": 5, &"blend_sharpness": 0.875,
	}
	for param: StringName in params:
		material.call(&"set_shader_param", param, params[param])
	ResourceSaver.save(material, folder.path_join("M_terrain.tres"))
	material.take_over_path(folder.path_join("M_terrain.tres"))
	var assets: Resource = ClassDB.instantiate(&"Terrain3DAssets")
	assets.resource_path = "res://maps/level.tscn::Terrain3DAssets_k2"
	terrain.set(&"assets", assets)
	TrenchDotTerrain3DCache.export(terrain, folder)
	var m := _manifest(folder)
	var expected := {
		"world_background": 2, "auto_shader": true,
		"world_noise": { "scale": 7.25, "height": -64.5, "offset": [12.5, -3.0, 0.75], "lod_distance": 9000.0, "max_octaves": 9, "min_octaves": 3, "fragment_normals": true },
		"auto": { "slope": 2.5, "height_reduction": 0.375, "base_texture": 3, "overlay_texture": 5 },
		"blend_sharpness": 0.875, "dual_scaling": true, "macro_variation": true, "projection": true,
	}
	var got: Dictionary = m.get("material", {})
	t.check(_same_values(got, expected), "a noise background exports its noise, auto shader and blend values, got %s" % [got])
	var text := FileAccess.get_file_as_string(TrenchDotTerrain3DCache.cache_dir(folder).path_join("manifest.json"))
	t.check(text.contains("\"max_octaves\": 9,") and text.contains("\"base_texture\": 3,"), "whole number values are written as integers for the editor")
	var saved := folder.path_join("M_terrain.tres")
	t.check(m.get("resources", {}) == { "material": saved, "assets": "", "ocean_material": "" }, "the resources name the saved material and leave out the scene's built in assets, got %s" % [m.get("resources")])

	var plain: Resource = ClassDB.instantiate(&"Terrain3DMaterial")
	plain.set(&"world_background", 0)
	terrain.set(&"material", plain)
	await t.process_frame
	TrenchDotTerrain3DCache.export(terrain, folder)
	got = _manifest(folder).get("material", {})
	t.check(_ints(got.get("world_background")) == 0 and got.get("auto_shader") == false and got.get("projection") == false, "no background writes the mode, got %s" % [got])
	t.check(_same_values(got, { "world_noise": { "scale": 5.0, "height": 32.0, "offset": [0.0, 0.0, 0.0], "lod_distance": 7500.0, "max_octaves": 4, "min_octaves": 2, "fragment_normals": false } }), "and the shader's noise defaults, got %s" % [got.get("world_noise")])
	t.check(_same_values(got, { "auto": { "slope": 1.0, "height_reduction": 0.1, "base_texture": 0, "overlay_texture": 1 } }) and t.near(got.get("blend_sharpness", -1.0), 0.5), "and the shader's auto shader and blend defaults, got %s" % [got])
	terrain.free()

## Whether dictionary [param got] has the keys of [param expected] with the same values, numbers within a little.
## Dictionaries inside have to match key for key.
static func _same_values(got: Variant, expected: Dictionary) -> bool:
	if not got is Dictionary:
		return false
	for key in expected:
		var want: Variant = expected[key]
		var have: Variant = got.get(key)
		if want is Dictionary:
			if not (have is Dictionary and have.size() == want.size() and _same_values(have, want)):
				return false
		elif want is Array:
			if not _near_all(have, want):
				return false
		elif want is bool:
			if have != want:
				return false
		elif not (have is float or have is int) or absf(float(have) - float(want)) > 0.001:
			return false
	return true

## The ocean of the Terrain3D node: Terrain3D's own ShaderMaterial gives its sea level and water color, a uniform a
## ShaderMaterial leaves unset takes its shader's default, a BaseMaterial3D gives its albedo color at sea level 0.
static func _test_ocean(t) -> void:
	var folder := WORK.path_join("ocean")
	var terrain := await _terrain(t, folder, 64, 1.0)
	terrain.get(&"data").add_region_blank(Vector2i(3, 1), false)
	TrenchDotTerrain3DCache.export(terrain, folder)
	var ocean: Dictionary = _manifest(folder).get("ocean", {})
	t.check(ocean.get("enabled") == false and t.near(ocean.get("sea_level", -1.0), 0.0) and _near_all(ocean.get("water_color"), [0.1058, 0.1568, 0.2117, 1.0]) and ocean.get("source") == "", "an ocean that was never on is off with the defaults, got %s" % [ocean])

	terrain.set(&"ocean_enabled", true)
	await t.process_frame
	TrenchDotTerrain3DCache.export(terrain, folder)
	var m := _manifest(folder)
	ocean = m.get("ocean", {})
	const OWN := "res://addons/terrain_3d/extras/shaders/M_ocean.tres"
	t.check(ocean.get("enabled") == true and t.near(ocean.get("sea_level", 0.0), 5.0) and _near_all(ocean.get("water_color"), [0.1058, 0.1568, 0.2117, 1.0]) and ocean.get("source") == OWN, "Terrain3D's own ocean material gives sea level 5, got %s" % [ocean])
	t.check(m.get("resources", {}).get("ocean_material") == OWN, "the resources name the ocean material, got %s" % [m.get("resources")])

	var shader := Shader.new()
	shader.code = "shader_type spatial;\nuniform float sea_level = 1.0;\nuniform vec3 water_color : source_color = vec3(0.25, 0.5, 0.75);\nvoid fragment() { ALBEDO = water_color; }\n"
	var water := ShaderMaterial.new()
	water.shader = shader
	water.set_shader_parameter(&"sea_level", -7.5)
	var saved := folder.path_join("water.tres")
	ResourceSaver.save(water, saved)
	water.take_over_path(saved)
	# Note: the headless renderer logs an error per ocean mesh for a material made in code, so the ocean goes off first.
	terrain.set(&"ocean_enabled", false)
	terrain.set(&"ocean_material", water)
	TrenchDotTerrain3DCache.export(terrain, folder)
	ocean = _manifest(folder).get("ocean", {})
	# Note: the headless renderer knows no shader defaults, there the water takes the color Terrain3D's ocean shader has.
	var known := RenderingServer.shader_get_parameter_default(shader.get_rid(), &"water_color") != null
	var color := [0.25, 0.5, 0.75, 1.0] if known else [0.1058, 0.1568, 0.2117, 1.0]
	t.check(ocean.get("enabled") == false and t.near(ocean.get("sea_level", 0.0), -7.5) and _near_all(ocean.get("water_color"), color) and ocean.get("source") == saved, "an ocean turned off still gives its ShaderMaterial's sea level and its shader's water color, got %s" % [ocean])

	var bare := ShaderMaterial.new()
	bare.shader = Shader.new()
	bare.shader.code = "shader_type spatial;\nuniform vec3 water_color = vec3(0.0);\nvoid fragment() { ALBEDO = water_color; }\n"
	bare.set_shader_parameter(&"water_color", Vector3(0.625, 0.375, 0.125))
	terrain.set(&"ocean_material", bare)
	TrenchDotTerrain3DCache.export(terrain, folder)
	ocean = _manifest(folder).get("ocean", {})
	t.check(t.near(ocean.get("sea_level", -1.0), 0.0) and _near_all(ocean.get("water_color"), [0.625, 0.375, 0.125, 1.0]) and ocean.get("source") == "", "a shader without a sea level puts the water at 0, got %s" % [ocean])

	var base := StandardMaterial3D.new()
	base.albedo_color = Color(0.125, 0.375, 0.625, 0.5)
	terrain.set(&"ocean_material", base)
	TrenchDotTerrain3DCache.export(terrain, folder)
	ocean = _manifest(folder).get("ocean", {})
	t.check(t.near(ocean.get("sea_level", -1.0), 0.0) and _near_all(ocean.get("water_color"), [0.125, 0.375, 0.625, 0.5]) and ocean.get("source") == "", "a BaseMaterial3D gives its albedo color at sea level 0, got %s" % [ocean])
	terrain.free()

	var older := Node3D.new()
	var none := { "material": "", "assets": "", "ocean_material": "" }
	t.check(TrenchDotTerrain3DCache._ocean(older) == null and TrenchDotTerrain3DCache._resources(older) == none and TrenchDotTerrain3DCache._number(older, &"detiling_shift", 0.25) == 0.25, "a node without the ocean, resources and detiling of Terrain3D 1.1 gets none of them")
	older.free()

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
	var look := {
		"material": { "world_background": 2.0, "auto_shader": true, "world_noise": { "scale": 7.25, "height": 64.0 }, "blend_sharpness": 0.875 },
		"textures": [{ "id": 1.0, "name": "Grass", "albedo_color": [0.75, 1.0, 0.5, 1.0], "thumbnail": "textures/1.png", "uv_scale": 0.25, "albedo": "textures/1_albedo.png", "hash": "5d41402abc4b2a76" }],
		"ocean": { "enabled": true, "sea_level": 5.25, "water_color": [0.25, 0.5, 0.75, 1.0], "source": "res://water/M_ocean.tres" },
		"resources": { "material": "res://data/M_terrain.tres", "assets": "res://data/assets.tres", "ocean_material": "res://water/M_ocean.tres" },
	}
	m.merge(look, true)
	FileAccess.open(folder.path_join(".trenchdot/manifest.json"), FileAccess.WRITE).store_string(JSON.stringify(m))
	var second := _cli(["export", folder])
	var header := _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-1, 2))))
	t.check(second[0] == 0 and float(_manifest(folder).get("vertex_spacing", 0.0)) == 2.5 and header.get("spacing") == 2.5, "the CLI keeps the vertex spacing of the last manifest, got %s %s" % [second, header.get("spacing")])
	m = _manifest(folder)
	for key in look:
		t.check(JSON.stringify(m.get(key), "", true) == JSON.stringify(look[key], "", true), "the CLI, which has no scene, keeps the %s of the last manifest, got %s" % [key, m.get(key)])

	var spaced := WORK.path_join("cli_spaced")
	_clear(spaced)
	DirAccess.make_dir_recursive_absolute(spaced)
	DirAccess.copy_absolute("res://tests/terrain3d/v1_0_2/terrain3d_02-01.res", spaced.path_join("terrain3d_02-01.res"))
	var third := _cli(["export", spaced])
	header = _decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(spaced, Vector2i(2, -1))))
	t.check(third[0] == 0 and float(_manifest(spaced).get("vertex_spacing", 0.0)) == 2.0 and header.get("spacing") == 2.0, "without a manifest the CLI takes the vertex spacing of the region files, got %s %s" % [_manifest(spaced).get("vertex_spacing"), header.get("spacing")])

	var missing := _cli(["export", WORK.path_join("nowhere")])
	t.check(missing[0] == 1 and missing[1].get("ok") == false and str(missing[1].get("error")).contains("nowhere"), "the CLI refuses a missing folder, got %s" % [missing])
	var usage := _cli(["import", folder])
	t.check(usage[0] == 1 and str(usage[1].get("error")).contains("usage"), "the CLI refuses an unknown command, got %s" % [usage])

	var cli: GDScript = load("res://addons/func_godot/src/trenchdot/terrain3d_cli.gd")
	var args := PackedStringArray(["export", folder])
	var absent: String = cli.refusal(args, &"NoSuchTerrain3D")
	t.check(absent.contains("not installed"), "the CLI refuses a project without Terrain3D, got %s" % [absent])
	t.check(cli.refusal(args) == "", "it accepts a project with Terrain3D")
