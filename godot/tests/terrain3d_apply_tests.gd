extends RefCounted
## The Terrain3D applier: edits TrenchDot leaves in <data folder>/.trenchdot/pending/ land in Terrain3D through its API,
## guarded by the base hash and the dirty flag of each region, saved and re-exported, and every pending file goes away.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const Export := preload("res://tests/terrain3d_export_tests.gd")
const WORK := "user://td_t3d_apply"
const SIZE := 64
const SPACING := 2.5
const TYPE_HEIGHT := 0
const TYPE_CONTROL := 1
const TYPE_COLOR := 2
const FORCE := 1
const REMOVE := 2
const NO_BASE := "0000000000000000000000000000000000000000000000000000000000000000"

static func run(t) -> void:
	print("- Terrain3D pending apply")
	if not ClassDB.class_exists("Terrain3D"):
		return
	Export._clear(WORK)
	_test_fixture_pending(t)
	await _test_guards(t)
	await _test_inspector(t)
	await _test_add_remove(t)
	await _test_remove_then_edit(t)
	await _test_bad_files(t)
	await _test_instances_follow(t)
	await _test_neighbours_stay_saved(t)
	await _test_neighbours_mid_stroke(t)
	await _test_apply_fills_missing_textures(t)
	_test_cli(t)
	await _test_cli_instances(t)
	await _test_cli_settings(t)
	await _test_settings_refusals(t)
	await _test_settings_save_fails(t)
	await _test_settings_keep_unloaded(t)
	await _test_scene_spacing(t)
	_test_new_folder_settings(t)
	await _test_lost_settings(t)
	await _test_spacing_sources(t)
	await _test_unsaved_settings(t)
	await _test_settings_overflow(t)
	_test_parse_region_file(t)
	await _test_reload_in_game(t)
	await _test_editor_watch(t)
	await _test_changed_while_closed(t)
	await load("res://tests/live_link_tests.gd").run_editor(t, "res://tests/editor/terrain3d_undo.tscn", "--td-terrain3d-undo", "TrenchDot edits land in a headless editor as one undo step")
	Export._clear(WORK)

## Three saved and exported regions off the origin: (-1, 0) with color, (3, -2) and (0, -1) without.
static func _world(t, name: String) -> Node3D:
	var folder := WORK.path_join(name)
	var terrain: Node3D = await Export._terrain(t, folder, SIZE, SPACING)
	for loc in [Vector2i(-1, 0), Vector2i(3, -2), Vector2i(0, -1)]:
		Export._fill(terrain, loc, Export._pattern(loc, loc == Vector2i(-1, 0)))
	terrain.get(&"data").update_maps(3, true, false)
	terrain.get(&"data").save_directory(folder)
	TrenchDotTerrain3DCache.export(terrain, folder)
	return terrain

## Heights that rise to the east and fall to the south, different per region.
static func _slanted(loc: Vector2i, lift: float) -> Image:
	var bytes := PackedByteArray()
	bytes.resize(SIZE * SIZE * 4)
	for z in SIZE:
		for x in SIZE:
			bytes.encode_float((z * SIZE + x) * 4, lift + loc.x * 3.0 + x * 0.75 - z * 0.3125)
	return Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, bytes)

## Writes a pending file the way crates/td_formats encode_pending does, from the maps of [param maps].
static func _pending(folder: String, loc: Vector2i, maps: Dictionary, base: String, flags := 0, size := SIZE) -> void:
	var path := TrenchDotTerrain3DCache.cache_dir(folder).path_join("pending").path_join(TrenchDotTerrain3DCache.region_stem(loc) + TrenchDotTerrain3DCache.REGION_EXT)
	DirAccess.make_dir_recursive_absolute(path.get_base_dir())
	FileAccess.open(path, FileAccess.WRITE).store_buffer(_pending_bytes(loc, size, SPACING, maps, base, flags))

## A pending file laid out the way crates/td_formats encode_pending writes one, from the images in [param maps].
static func _pending_bytes(loc: Vector2i, size: int, spacing: float, maps: Dictionary, base: String, flags := 0) -> PackedByteArray:
	var color: Image = maps.get("color")
	var payload: PackedByteArray = maps["height"].get_data() + maps["control"].get_data() + (color.get_data() if color else PackedByteArray())
	var head := PackedByteArray()
	head.resize(60)
	head.encode_u32(0, "TD3R".to_ascii_buffer().decode_u32(0))
	head.encode_u32(4, 1)
	head.encode_s32(8, loc.x)
	head.encode_s32(12, loc.y)
	head.encode_u32(16, size)
	head.encode_float(20, spacing)
	head.encode_u32(24, 1 if color else 0)
	var trailer := "BASE".to_ascii_buffer() + base.hex_decode()
	trailer.resize(40)
	trailer.encode_u32(36, flags)
	return head + payload.compress(FileAccess.COMPRESSION_ZSTD) + trailer

## The first mip level of [param image], Terrain3D gives a color map mipmaps.
static func _level0(image: Image) -> PackedByteArray:
	if not image:
		return PackedByteArray()
	var copy := image.duplicate() as Image
	copy.clear_mipmaps()
	return copy.get_data()

static func _hash(folder: String, loc: Vector2i) -> String:
	for entry in TrenchDotTerrain3DCache.read_manifest(folder).get("regions", []):
		if Vector2i(int(entry["loc"][0]), int(entry["loc"][1])) == loc:
			return entry["hash"]
	return ""

static func _pending_left(folder: String) -> PackedStringArray:
	return DirAccess.get_files_at(TrenchDotTerrain3DCache.cache_dir(folder).path_join("pending"))

## The height map Terrain3D saved for [param loc], read back from its .res file.
static func _saved_height(folder: String, loc: Vector2i) -> Image:
	var path := folder.path_join(TrenchDotTerrain3DCache.region_stem(loc) + ".res")
	if not FileAccess.file_exists(path):
		return null
	var region: Resource = ResourceLoader.load(path, "", ResourceLoader.CACHE_MODE_IGNORE)
	return region.call(&"get_map", TYPE_HEIGHT) if region else null

static func _test_fixture_pending(t) -> void:
	var edit := TrenchDotTerrain3DCache.decode_pending(Export._fixture("pending/terrain3d_03-02.td3r"))
	var lifted := Export._pattern(Vector2i(3, -2), false)
	var want := PackedByteArray()
	want.resize(SIZE * SIZE * 4)
	for i in SIZE * SIZE:
		want.encode_float(i * 4, lifted["height"].get_data().decode_float(i * 4) + 1.0)
	var region_hash: String = Export._decode(Export._fixture("regions/terrain3d_03-02.td3r")).get("hash")
	t.check(edit.get("loc") == Vector2i(3, -2) and edit.get("size") == SIZE and edit.get("force") == true and edit.get("remove") == false, "the Rust pending fixture decodes its location, size and flags, got %s" % [{ "loc": edit.get("loc"), "size": edit.get("size"), "force": edit.get("force"), "remove": edit.get("remove"), "problem": edit.get("problem") }])
	t.check(edit.get("base") == region_hash and edit.get("color") == PackedByteArray(), "its base is the hash of the fixture region and it has no color")
	t.check(edit.get("heights") == want and edit.get("control") == lifted["control"].get_data(), "its heights are the fixture lifted by 1 m and its control words are kept bit for bit")

static func _test_guards(t) -> void:
	var folder := WORK.path_join("guards")
	var terrain := await _world(t, "guards")
	var data: Object = terrain.get(&"data")
	var west := Vector2i(-1, 0)
	var south := Vector2i(3, -2)
	var north := Vector2i(0, -1)

	var empty := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(empty.ok and empty.accepted.is_empty() and empty.refused.is_empty() and TrenchDotTerrain3DCache.read_manifest(folder).get("applied", []).is_empty(), "with no pending file apply does nothing and reports nothing, got %s" % [empty])

	var west_maps := Export._pattern(west, true)
	west_maps["height"] = _slanted(west, 7.0)
	west_maps["color"].fill(Color(0.2, 0.6, 0.4, 0.9))
	_pending(folder, west, west_maps, _hash(folder, west))
	var stale := _hash(folder, south)
	data.get_region(south).get_map(TYPE_HEIGHT).set_pixel(5, 9, Color(-40.0, 0, 0))
	data.get_region(south).set(&"modified", true)
	data.save_region(south, folder)
	_pending(folder, south, { "height": _slanted(south, -3.0), "control": Export._pattern(south, false)["control"] }, stale)
	var first := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(first.ok and Export._ints(first.accepted) == [[-1, 0]] and Export._ints(first.refused) == [{ "loc": [3, -2], "reason": "changed" }], "an edit of a region that changed since is refused while another region still applies, got %s" % [first])
	var west_region: Object = data.get_region(west)
	t.check(west_region.get_map(TYPE_HEIGHT).get_data() == west_maps["height"].get_data() and west_region.get_map(TYPE_CONTROL).get_data() == west_maps["control"].get_data() and _level0(west_region.get_map(TYPE_COLOR)) == west_maps["color"].get_data(), "the accepted region holds the pending heights, control and color")
	t.check(t.near(data.get_height(Vector3(-SIZE * SPACING + 10 * SPACING, 0, 4 * SPACING)), 7.0 - 3.0 + 10 * 0.75 - 4 * 0.3125), "Terrain3D samples the new height at a vertex of the accepted region")
	t.check(not west_region.get(&"modified") and _saved_height(folder, west).get_data() == west_maps["height"].get_data(), "the accepted region is saved to its file and no longer dirty")
	t.check(t.near(data.get_region(south).get_map(TYPE_HEIGHT).get_pixel(5, 9).r, -40.0), "the refused region keeps Godot's height")
	t.check(_hash(folder, west) == TrenchDotTerrain3DCache.region_hash(west_region, west) and _hash(folder, west) != _hash(folder, north), "the cache now carries the accepted region's new hash")
	t.check(_hash(folder, south) == TrenchDotTerrain3DCache.region_hash(data.get_region(south), south) and _hash(folder, south) != stale, "the refused region is re-exported so TrenchDot sees Godot's version")
	t.check(_pending_left(folder).is_empty(), "no pending file remains, got %s" % [_pending_left(folder)])
	var report: Array = TrenchDotTerrain3DCache.read_manifest(folder).get("applied", [])
	t.check(report.size() == 1 and int(report[0]["seq"]) == 1 and Export._ints(report[0]["accepted"]) == [[-1, 0]] and str(report[0]["refused"][0]["reason"]) == "changed", "the manifest reports the apply as seq 1, got %s" % [report])

	_pending(folder, south, { "height": _slanted(south, -3.0), "control": Export._pattern(south, false)["control"] }, stale, FORCE)
	var forced := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(forced.accepted) == [[3, -2]] and forced.refused.is_empty(), "a forced edit applies over a changed region, got %s" % [forced])
	var south_color: Image = data.get_region(south).get_map(TYPE_COLOR)
	t.check(data.get_region(south).get_map(TYPE_HEIGHT).get_data() == _slanted(south, -3.0).get_data(), "it replaces Godot's heights")
	t.check(not south_color or Vector4(1, 1, 1, 0.5).distance_to(Vector4(south_color.get_pixel(17, 40).r, south_color.get_pixel(17, 40).g, south_color.get_pixel(17, 40).b, south_color.get_pixel(17, 40).a)) < 0.01, "it leaves the blank color of a region alone when the pending file has no color")

	var north_region: Object = data.get_region(north)
	north_region.get_map(TYPE_HEIGHT).set_pixel(63, 0, Color(12.0, 0, 0))
	north_region.set(&"modified", true)
	_pending(folder, north, { "height": _slanted(north, 1.0), "control": Export._pattern(north, false)["control"] }, TrenchDotTerrain3DCache.region_hash(north_region, north))
	var dirty := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(dirty.accepted.is_empty() and Export._ints(dirty.refused) == [{ "loc": [0, -1], "reason": "dirty" }] and t.near(north_region.get_map(TYPE_HEIGHT).get_pixel(63, 0).r, 12.0), "an edit of a region with unsaved Godot edits is refused, even with the right base, got %s" % [dirty])
	_pending(folder, north, { "height": _slanted(north, 1.0), "control": Export._pattern(north, false)["control"] }, TrenchDotTerrain3DCache.region_hash(north_region, north))
	var unchecked := TrenchDotTerrain3DCache.apply(terrain, folder, { "check_dirty": false })
	t.check(Export._ints(unchecked.accepted) == [[0, -1]] and north_region.get_map(TYPE_HEIGHT).get_data() == _slanted(north, 1.0).get_data(), "without the dirty check the same edit applies, got %s" % [unchecked])
	north_region.set(&"modified", true)
	_pending(folder, north, { "height": _slanted(north, 2.0), "control": Export._pattern(north, false)["control"] }, NO_BASE, FORCE)
	var forced_dirty := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(forced_dirty.accepted) == [[0, -1]] and not north_region.get(&"modified"), "a forced edit applies over unsaved Godot edits, got %s" % [forced_dirty])
	var seqs: Array = TrenchDotTerrain3DCache.read_manifest(folder).get("applied", []).map(func(a): return int(a["seq"]))
	t.check(seqs == [1, 2, 3, 4, 5], "each apply adds a report with the next seq, got %s" % [seqs])
	t.check(_pending_left(folder).is_empty(), "no pending file remains after the guarded applies")
	terrain.free()

## The lines of the slot's own Inspector section, by name, the way Godot decides to draw them: its category has to
## hold at least one editor property, or the whole section is skipped.
static func _inspector_lines(slot: Node) -> Dictionary:
	var lines := {}
	var inside := false
	for prop in slot.get_property_list():
		if prop["usage"] & PROPERTY_USAGE_CATEGORY:
			inside = prop["hint_string"] == slot.get_script().resource_path
		elif inside and prop["usage"] & PROPERTY_USAGE_EDITOR and not str(prop["name"]).begins_with("metadata/"):
			lines[prop["name"]] = prop["usage"]
	return lines

static func _test_inspector(t) -> void:
	var folder := WORK.path_join("inspector")
	var terrain := await _world(t, "inspector")
	var slot := await _slot(t, terrain)
	slot.set_meta(TrenchDotTerrain3D.LINK_META, 40)
	var lines := _inspector_lines(slot)
	t.check(not lines.is_empty(), "the slot draws a section of its own in the Inspector, got %s" % [lines])
	var names := lines.keys()
	names.sort()
	t.check(names == ["trenchdot_cache", "trenchdot_data_folder", "trenchdot_last_apply", "trenchdot_linked_node", "trenchdot_pending_edits"], "the section has the link, folder, pending, last apply and cache lines, got %s" % [names])
	t.check(not lines.is_empty() and lines.values().all(func(usage: int) -> bool: return usage & PROPERTY_USAGE_READ_ONLY and not usage & PROPERTY_USAGE_STORAGE), "every line is read only and never stored")
	t.check(str(slot.get(&"trenchdot_linked_node")).contains("40") and slot.get(&"trenchdot_data_folder") == folder, "it names the linked node and the data folder, got %s and %s" % [slot.get(&"trenchdot_linked_node"), slot.get(&"trenchdot_data_folder")])
	t.check(slot.get(&"trenchdot_pending_edits") == 0 and not str(slot.get(&"trenchdot_last_apply")).contains("#"), "with nothing pending and nothing applied it shows no apply, got %s and %s" % [slot.get(&"trenchdot_pending_edits"), slot.get(&"trenchdot_last_apply")])
	t.check(str(slot.get(&"trenchdot_cache")).begins_with("3 "), "the cache line counts the 3 exported regions, got %s" % [slot.get(&"trenchdot_cache")])

	var west := Vector2i(-1, 0)
	var south := Vector2i(3, -2)
	var north := Vector2i(0, -1)
	var data: Object = terrain.get(&"data")
	_pending(folder, west, Export._pattern(west, true), NO_BASE.replace("0", "1"))
	_pending(folder, north, { "height": _slanted(north, 4.0), "control": Export._pattern(north, false)["control"] }, _hash(folder, north))
	_pending(folder, south, Export._pattern(south, false), NO_BASE.replace("0", "2"))
	t.check(slot.get(&"trenchdot_pending_edits") == 0, "the lines keep what they read until the next refresh, got %s" % [slot.get(&"trenchdot_pending_edits")])
	slot.refresh_status()
	t.check(slot.get(&"trenchdot_pending_edits") == 3, "three pending files show as 3 pending edits, got %s" % [slot.get(&"trenchdot_pending_edits")])
	slot.apply_pending()
	var last := str(slot.get(&"trenchdot_last_apply"))
	t.check(slot.get(&"trenchdot_pending_edits") == 0 and last.begins_with("#1") and last.contains("1 applied") and last.contains("2 refused"), "after the apply nothing is pending and the last apply names 1 applied and 2 refused, got %s" % [last])
	t.check(last.contains("changed at -1, 0") and last.contains("changed at 3, -2"), "it names the reason and location of each refused region, got %s" % [last])

	_pending(folder, west, Export._pattern(west, true), NO_BASE.replace("0", "3"))
	slot.apply_pending()
	last = str(slot.get(&"trenchdot_last_apply"))
	t.check(last.begins_with("#2") and last.contains("0 applied") and last.contains("1 refused") and last.contains("changed at -1, 0") and not last.contains("3, -2"), "a second apply replaces the line with its own report, got %s" % [last])

	# A slot that never ran _ready is what a script reload leaves, its lines still come from disk.
	var other_terrain := await _world(t, "inspector_other")
	var other := TrenchDotTerrain3D.new()
	other_terrain.get_parent().remove_child(other_terrain)
	other.add_child(other_terrain)
	_pending(WORK.path_join("inspector_other"), south, Export._pattern(south, false), NO_BASE)
	var got := [other.get(&"trenchdot_linked_node"), other.get(&"trenchdot_pending_edits"), other.get(&"trenchdot_last_apply"), other.get(&"trenchdot_cache")]
	t.check(got[0] == "none" and got[1] == 1 and got[2] == "none yet" and str(got[3]).begins_with("3 regions exported, 0 "), "a second slot reads its own state on the first look, got %s" % [got])
	other.free()

	# Only the slot is owned, like a scene a user saved, the Terrain3D under it is not part of the pack.
	slot.owner = slot.get_parent()
	var packed := PackedScene.new()
	packed.pack(slot.get_parent())
	var state := packed.get_state()
	var stored := []
	for i in state.get_node_property_count(1):
		stored.append(str(state.get_node_property_name(1, i)))
	t.check(stored.has("metadata/%s" % TrenchDotTerrain3D.LINK_META) and not stored.any(func(n: String) -> bool: return n.begins_with("trenchdot_")), "the scene keeps the link and none of the section's lines, got %s" % [stored])
	slot.get_parent().free()

static func _test_add_remove(t) -> void:
	var folder := WORK.path_join("add_remove")
	var terrain := await _world(t, "add_remove")
	var data: Object = terrain.get(&"data")
	var added := Vector2i(-16, 15)
	var north := Vector2i(0, -1)
	var south := Vector2i(3, -2)
	_pending(folder, added, { "height": _slanted(added, 20000.0), "control": Export._pattern(added, false)["control"] }, NO_BASE)
	_pending(folder, north, Export._pattern(north, false), _hash(folder, north), REMOVE)
	_pending(folder, south, { "height": _slanted(south, 0.0), "control": Export._pattern(south, false)["control"] }, NO_BASE)
	var result := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(result.accepted) == [[-16, 15], [0, -1]] and Export._ints(result.refused) == [{ "loc": [3, -2], "reason": "changed" }], "a new region and a removal apply, a new region over an existing one is refused, got %s" % [result])
	t.check(data.has_region(added) and data.get_region(added).get_map(TYPE_HEIGHT).get_data() == _slanted(added, 20000.0).get_data() and FileAccess.file_exists(folder.path_join("terrain3d-16_15.res")), "the new region at the corner of the world is added and saved")
	var range_: Vector2 = data.get_region(added).get(&"height_range")
	t.check(t.near(range_.x, 20000.0 - 48.0 - 63 * 0.3125) and t.near(range_.y, 20000.0 - 48.0 + 63 * 0.75), "Terrain3D's height range follows the new heights, got %s" % [range_])
	t.check(not data.has_region(north) and not FileAccess.file_exists(folder.path_join("terrain3d_00-01.res")) and not FileAccess.file_exists(TrenchDotTerrain3DCache.region_path(folder, north)), "the removed region is gone from Terrain3D, its file and the cache")
	var locs: Array = Export._ints(TrenchDotTerrain3DCache.read_manifest(folder).get("regions", []).map(func(r): return r["loc"]))
	t.check(locs == [[3, -2], [-1, 0], [-16, 15]], "the manifest lists the regions after the apply, got %s" % [locs])
	_pending(folder, north, Export._pattern(north, false), NO_BASE, REMOVE)
	var again := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(again.accepted) == [[0, -1]] and again.problems.is_empty(), "removing a region that is already gone is accepted and harmless, got %s" % [again])
	t.check(_pending_left(folder).is_empty(), "no pending file remains after adding and removing")
	terrain.free()

## Removing a region renumbers Terrain3D's regions until its maps update, the regions after it must still be found.
static func _test_remove_then_edit(t) -> void:
	var folder := WORK.path_join("remove_then_edit")
	var terrain := await _world(t, "remove_then_edit")
	var data: Object = terrain.get(&"data")
	var west := Vector2i(-1, 0)
	var south := Vector2i(3, -2)
	var north := Vector2i(0, -1)
	_pending(folder, west, Export._pattern(west, false), _hash(folder, west), REMOVE)
	_pending(folder, south, { "height": _slanted(south, 2.0), "control": Export._pattern(south, false)["control"] }, _hash(folder, south))
	_pending(folder, north, { "height": _slanted(north, -3.0), "control": Export._pattern(north, false)["control"] }, _hash(folder, north))
	var result := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(result.accepted) == [[-1, 0], [0, -1], [3, -2]] and result.refused.is_empty(), "the regions after a removed one still apply, got %s" % [result])
	t.check(not data.has_region(west) and data.get_region(south).get_map(TYPE_HEIGHT).get_data() == _slanted(south, 2.0).get_data() and data.get_region(north).get_map(TYPE_HEIGHT).get_data() == _slanted(north, -3.0).get_data(), "each edit lands in its own region")
	t.check(Array(data.get_region_locations()).duplicate().map(func(l: Vector2i) -> Array: return [l.x, l.y]) == [[3, -2], [0, -1]], "only the removed region is gone, got %s" % [data.get_region_locations()])
	terrain.free()

static func _test_bad_files(t) -> void:
	var folder := WORK.path_join("bad")
	var terrain := await _world(t, "bad")
	var west := Vector2i(-1, 0)
	var pending := TrenchDotTerrain3DCache.cache_dir(folder).path_join("pending")
	var big := Image.create_empty(128, 128, false, Image.FORMAT_RF)
	_pending(folder, west, { "height": big, "control": big }, _hash(folder, west), 0, 128)
	var size := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(size.accepted.is_empty() and Export._ints(size.refused) == [{ "loc": [-1, 0], "reason": "size" }], "an edit for another region size is refused, got %s" % [size])

	FileAccess.open(pending.path_join("terrain3d_00-01.td3r"), FileAccess.WRITE).store_buffer("TD3Rjunk".to_ascii_buffer())
	_pending(folder, Vector2i(3, -2), Export._pattern(Vector2i(3, -2), false), _hash(folder, Vector2i(3, -2)))
	var newer := FileAccess.get_file_as_bytes(pending.path_join("terrain3d_03-02.td3r"))
	newer.encode_u32(4, 2)
	FileAccess.open(pending.path_join("terrain3d_03-02.td3r"), FileAccess.WRITE).store_buffer(newer)
	var no_trailer := FileAccess.get_file_as_bytes(pending.path_join("terrain3d_03-02.td3r"))
	no_trailer.encode_u32(4, 1)
	no_trailer.encode_u32(no_trailer.size() - 40, 0)
	FileAccess.open(pending.path_join("terrain3d-01_00.td3r"), FileAccess.WRITE).store_buffer(no_trailer)
	FileAccess.open(pending.path_join("notes.txt"), FileAccess.WRITE).store_string("left alone")
	var bad := TrenchDotTerrain3DCache.apply(terrain, folder)
	var problems := "\n".join(bad.problems)
	t.check(bad.ok and bad.accepted.is_empty() and bad.refused.is_empty() and bad.problems.size() == 3, "broken pending files are reported as problems, got %s" % [bad])
	t.check(problems.contains("not a TrenchDot pending region") and problems.contains("newer than 1") and problems.contains("no BASE trailer"), "each problem says what is wrong, got %s" % [problems])
	t.check(_pending_left(folder) == PackedStringArray(["notes.txt"]), "broken pending files are removed and other files are left alone, got %s" % [_pending_left(folder)])
	t.check(terrain.get(&"data").get_region(west).get_map(TYPE_HEIGHT).get_data() == Export._pattern(west, true)["height"].get_data(), "no broken file changes a region")
	terrain.free()

static func _test_instances_follow(t) -> void:
	var folder := WORK.path_join("instances")
	var terrain := await _world(t, "instances")
	var assets: Object = terrain.get(&"assets")
	if assets.call(&"get_mesh_count") == 0:
		assets.call(&"set_mesh_asset", 0, ClassDB.instantiate(&"Terrain3DMeshAsset"))
	var offset := float(assets.call(&"get_mesh_asset", 0).get(&"height_offset"))
	var instancer: Object = terrain.get(&"instancer")
	var west := Vector2i(-1, 0)
	var spots := [Vector3(-150.0, 0.0, 22.5), Vector3(-37.5, 0.0, 101.25)]
	var xforms: Array[Transform3D] = []
	for spot in spots:
		xforms.append(Transform3D(Basis(), spot + Vector3.UP * terrain.get(&"data").get_height(spot)))
	instancer.call(&"add_transforms", 0, xforms)
	terrain.get(&"data").save_directory(folder)
	# Note: the fixture control words set the hole bit at random and Terrain3D drops an instance over a hole, so the
	# edit clears every control word.
	var maps := { "height": _slanted(west, 30.0), "control": Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF) }
	_pending(folder, west, maps, _hash(folder, west))
	var result := TrenchDotTerrain3DCache.apply(terrain, folder)
	var ys: Array = []
	for cells in terrain.get(&"data").get_region(west).get(&"instances").values():
		for cell in cells.values():
			for xform: Transform3D in cell[0]:
				ys.append(xform.origin.y)
	ys.sort()
	var want := spots.map(func(s: Vector3) -> float: return terrain.get(&"data").get_surface_height(s) + offset)
	want.sort()
	t.check(Export._ints(result.accepted) == [[-1, 0]] and ys.size() == 2 and t.near(ys[0], want[0]) and t.near(ys[1], want[1]), "Terrain3D instances follow the applied height, got %s want %s from %s" % [ys, want, result])
	terrain.free()

## A saved and exported Terrain3D with the fixture maps at [param locs] and an instance at each of [param spots]. The
## control maps are cleared, a fixture hole would drop an instance.
static func _neighbourhood(t, folder: String, locs: Array, spots: Array) -> Node3D:
	var terrain: Node3D = await Export._terrain(t, folder, SIZE, SPACING)
	var data: Object = terrain.get(&"data")
	for loc: Vector2i in locs:
		Export._fill(terrain, loc, Export._pattern(loc, false))
		data.get_region(loc).set_map(TYPE_CONTROL, Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF))
	data.update_maps(3, true, false)
	if not spots.is_empty():
		var assets: Object = terrain.get(&"assets")
		if assets.call(&"get_mesh_count") == 0:
			assets.call(&"set_mesh_asset", 0, ClassDB.instantiate(&"Terrain3DMeshAsset"))
		var xforms: Array[Transform3D] = []
		for spot: Vector3 in spots:
			xforms.append(Transform3D(Basis(), spot + Vector3.UP * data.get_height(spot)))
		terrain.get(&"instancer").call(&"add_transforms", 0, xforms)
	data.save_directory(folder)
	TrenchDotTerrain3DCache.export(terrain, folder)
	return terrain

## Leaves a pending edit that makes [param loc] a slope lifted by [param lift] without holes.
static func _send(folder: String, loc: Vector2i, lift: float) -> void:
	_pending(folder, loc, { "height": _slanted(loc, lift), "control": Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF) }, _hash(folder, loc))

## Terrain3D's update_transforms looks past the region it is given, an apply or a restore must not leave the regions
## around it flagged unsaved.
static func _test_neighbours_stay_saved(t) -> void:
	var folder := WORK.path_join("neighbours")
	var corner := Vector2i(-1, 0)
	var west := Vector2i(-2, 0)
	var north := Vector2i(-1, -1)
	var locs := [Vector2i(-2, -1), north, west, corner, Vector2i(2, 1)]
	# One instance in each neighbour within the 1 m Terrain3D looks past the corner, one inside the corner.
	var spots := [Vector3(-160.5, 0.0, 41.0), Vector3(-97.0, 0.0, -0.5), Vector3(-160.5, 0.0, -0.5), Vector3(-83.0, 0.0, 77.0)]
	var terrain := await _neighbourhood(t, folder, locs, spots)
	var data: Object = terrain.get(&"data")
	var instances := locs.map(func(loc: Vector2i) -> String: return var_to_str(data.get_region(loc).get(&"instances")))
	var unsaved := func() -> Array: return locs.filter(func(loc: Vector2i) -> bool: return data.get_region(loc).get(&"modified"))
	_send(folder, corner, 9.0)
	var result := TrenchDotTerrain3DCache.apply(terrain, folder)
	var moved := locs.filter(func(loc: Vector2i) -> bool: return var_to_str(data.get_region(loc).get(&"instances")) != instances[locs.find(loc)])
	t.check(Export._ints(result.accepted) == [[-1, 0]] and unsaved.call().is_empty() and TrenchDotTerrain3DCache.read_manifest(folder).get("dirty", []).is_empty() and moved == [corner], "an apply leaves the regions around it saved and their instances where they were, got %s unsaved and %s moved from %s" % [unsaved.call(), moved, result])
	data.get_region(north).set(&"modified", true)
	_send(folder, corner, 3.0)
	result = TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(result.accepted) == [[-1, 0]] and unsaved.call() == [north], "a neighbour Godot has unsaved stays unsaved through an apply, got %s from %s" % [unsaved.call(), result])
	_send(folder, north, 4.0)
	_send(folder, west, 6.0)
	result = TrenchDotTerrain3DCache.apply(terrain, folder)
	var refused: Array = result.refused.map(func(r: Dictionary) -> Array: return [Export._ints(r["loc"]), r["reason"]])
	t.check(Export._ints(result.accepted) == [[-2, 0]] and refused == [[[-1, -1], "dirty"]], "a send to a saved neighbour of an applied region is accepted and one to an unsaved neighbour refused, got %s" % [result])
	data.save_directory(folder)
	var before: Array = TrenchDotTerrain3DCache.snapshot_region(data, west)
	var skipped := TrenchDotTerrain3DCache.restore_regions(terrain, { corner: [_slanted(corner, 2.0), Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF), data.get_region(corner).get_map(TYPE_COLOR).duplicate()] })
	t.check(skipped.is_empty() and unsaved.call() == [corner] and TrenchDotTerrain3DCache.snapshot_region(data, west)[0].get_data() == before[0].get_data(), "a restore flags only the regions it puts back, got %s unsaved and %s skipped" % [unsaved.call(), skipped])
	terrain.free()

## An apply leaves the texture list of the last export alone, unless that export listed none because the assets had no
## textures yet.
static func _test_apply_fills_missing_textures(t) -> void:
	var folder := WORK.path_join("missing_textures")
	var terrain := await _world(t, "missing_textures")
	t.check(TrenchDotTerrain3DCache.read_manifest(folder).get("textures", [-1]).is_empty(), "the world starts with no textures")
	var names := func() -> Array: return TrenchDotTerrain3DCache.read_manifest(folder).get("textures", []).map(func(e: Dictionary) -> String: return e["name"])
	terrain.set(&"assets", Export._folder_look(folder)[1])
	_send(folder, Vector2i(3, -2), 5.0)
	var result := TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(result.accepted) == [[3, -2]] and names.call() == ["Cliff", "Grass"], "an apply lists the textures a manifest has none of, got %s from %s" % [names.call(), result])
	var sand: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
	sand.set(&"name", "Sand")
	terrain.get(&"assets").call(&"set_texture_asset", 0, sand)
	_send(folder, Vector2i(0, -1), 2.0)
	result = TrenchDotTerrain3DCache.apply(terrain, folder)
	t.check(Export._ints(result.accepted) == [[0, -1]] and names.call() == ["Cliff", "Grass"], "an apply keeps a listed set until an export asks for textures, got %s from %s" % [names.call(), result])
	terrain.free()

## Terrain3D flags a region unsaved only when a brush stroke first reaches it. A neighbour an apply drew into a held
## stroke must stay flagged, or the rest of the stroke in it is not saved with the scene.
static func _test_neighbours_mid_stroke(t) -> void:
	var folder := WORK.path_join("mid_stroke")
	var corner := Vector2i(-1, 0)
	var north_west := Vector2i(-2, -1)
	var terrain := await _neighbourhood(t, folder, [north_west, Vector2i(-1, -1), Vector2i(-2, 0), corner], [])
	var data: Object = terrain.get(&"data")
	var editor: Object = ClassDB.instantiate(&"Terrain3DEditor")
	editor.call(&"set_terrain", terrain)
	terrain.call(&"set_editor", editor)
	editor.call(&"start_operation", Vector3(-83.0, 0.0, 77.0))
	_send(folder, corner, 9.0)
	var result := TrenchDotTerrain3DCache.apply(terrain, folder)
	# What Terrain3DEditor does for each pixel a stroke paints.
	var region: Object = data.get_region(north_west)
	editor.call(&"backup_region", region)
	region.get_map(TYPE_HEIGHT).set_pixel(13, 5, Color(77.5, 0.0, 0.0))
	data.save_directory(folder)
	var saved := _saved_height(folder, north_west)
	var height: Variant = saved.get_pixel(13, 5).r if saved else null
	t.check(Export._ints(result.accepted) == [[-1, 0]] and height != null and t.near(height, 77.5), "a stroke held through an apply is saved in a neighbour it reaches afterwards, got %s from %s" % [height, result])
	terrain.call(&"set_editor", null)
	terrain.free()
	editor.free()

static func _test_cli(t) -> void:
	var folder := WORK.path_join("cli")
	Export._clear(folder)
	DirAccess.make_dir_recursive_absolute(folder)
	DirAccess.copy_absolute("res://tests/terrain3d/probe/terrain3d-01_02.res", folder.path_join("terrain3d-01_02.res"))
	var exported := Export._cli(["export", folder])
	var loc := Vector2i(-1, 2)
	var base := _hash(folder, loc)
	var region: Resource = ResourceLoader.load(folder.path_join("terrain3d-01_02.res"), "", ResourceLoader.CACHE_MODE_IGNORE)
	var size := int(region.get(&"region_size"))
	var heights := PackedByteArray()
	heights.resize(size * size * 4)
	for i in size * size:
		heights.encode_float(i * 4, -2.5 + (i % size) * 0.0625)
	var maps := { "height": Image.create_from_data(size, size, false, Image.FORMAT_RF, heights), "control": region.call(&"get_map", TYPE_CONTROL) }
	var path := TrenchDotTerrain3DCache.cache_dir(folder).path_join("pending").path_join(TrenchDotTerrain3DCache.region_stem(loc) + TrenchDotTerrain3DCache.REGION_EXT)
	DirAccess.make_dir_recursive_absolute(path.get_base_dir())
	FileAccess.open(path, FileAccess.WRITE).store_buffer(_pending_bytes(loc, size, 1.0, maps, base))
	var applied := Export._cli(["apply", folder])
	t.check(exported[0] == 0 and applied[0] == 0 and applied[1].get("ok") == true and Export._ints(applied[1].get("accepted")) == [[-1, 2]], "the CLI applies a pending edit to a saved folder, got %s" % [applied])
	var saved := _saved_height(folder, loc)
	t.check(saved and saved.get_data() == heights and _hash(folder, loc) != base and _pending_left(folder).is_empty(), "the region file holds the new heights, the cache its new hash and no pending file remains")
	var nothing := Export._cli(["apply", folder])
	t.check(nothing[0] == 0 and nothing[1].get("accepted") == [] and nothing[1].get("refused") == [], "the CLI with nothing pending succeeds and applies nothing, got %s" % [nothing])

## The CLI has no scene, so its Terrain3D lacks the meshes the instances of a region use and must leave them alone.
static func _test_cli_instances(t) -> void:
	var folder := WORK.path_join("cli_instances")
	var terrain := await _world(t, "cli_instances")
	var west := Vector2i(-1, 0)
	_add_instance(terrain)
	terrain.get(&"data").save_directory(folder)
	TrenchDotTerrain3DCache.export(terrain, folder)
	var base := _hash(folder, west)
	terrain.free()
	var maps := { "height": _slanted(west, 30.0), "control": Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF) }
	_pending(folder, west, maps, base)
	_pending(folder, Vector2i(2, 0), { "height": _slanted(Vector2i(2, 0), 5.0), "control": Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF) }, NO_BASE)
	var applied := Export._cli(["apply", folder])
	var accepted: Array = Export._ints(applied[1].get("accepted", []))
	accepted.sort()
	t.check(applied[0] == 0 and accepted == [[-1, 0], [2, 0]], "the CLI applies edits to a region with instances and a new region, got %s" % [applied])
	var saved := _saved_height(folder, west)
	t.check(saved != null and saved.get_data() == maps["height"].get_data() and _pending_left(folder).is_empty(), "the region with instances holds the new heights and no pending file remains")
	var region: Resource = ResourceLoader.load(folder.path_join("terrain3d-01_00.res"), "", ResourceLoader.CACHE_MODE_IGNORE)
	var count := 0
	for cells in region.get(&"instances").values():
		for cell in cells.values():
			count += cell[0].size()
	t.check(count == 1, "the instances of the region are kept, got %d" % count)

## Gives [param terrain] mesh assets and one instance of mesh 2 in region (-1, 0).
static func _add_instance(terrain: Node3D) -> void:
	var assets: Object = terrain.get(&"assets")
	for id in 3:
		if assets.call(&"get_mesh_count") <= id:
			assets.call(&"set_mesh_asset", id, ClassDB.instantiate(&"Terrain3DMeshAsset"))
	var spot := Vector3(-150.0, 0.0, 22.5)
	var xforms: Array[Transform3D] = [Transform3D(Basis(), spot + Vector3.UP * terrain.get(&"data").get_height(spot))]
	terrain.get(&"instancer").call(&"add_transforms", 2, xforms)

## Every file below [param folder] by its path under it, with its MD5. Dot folders like .trenchdot count, which
## [method DirAccess.get_directories_at] skips on Linux.
static func _tree(folder: String, below := "") -> Dictionary:
	var out := {}
	var path := folder.path_join(below)
	var dir := DirAccess.open(path)
	if dir == null:
		return out
	dir.include_hidden = true
	for file in dir.get_files():
		out[below.path_join(file)] = FileAccess.get_md5(path.path_join(file))
	for sub in dir.get_directories():
		out.merge(_tree(folder, below.path_join(sub)))
	return out

## The Terrain3D region files of [param folder] by name, with the region size and vertex spacing saved in each.
static func _region_files(folder: String) -> Dictionary:
	var out := {}
	for file in DirAccess.get_files_at(folder):
		if TrenchDotTerrain3DCache.parse_region_file(file) != null:
			var region: Resource = ResourceLoader.load(folder.path_join(file), "", ResourceLoader.CACHE_MODE_IGNORE)
			out[file] = [int(region.get(&"region_size")), float(region.get(&"vertex_spacing"))]
	return out

## The hash the manifest of [param folder] names for each region.
static func _hashes(folder: String) -> Dictionary:
	var out := {}
	for entry in TrenchDotTerrain3DCache.read_manifest(folder).get("regions", []):
		out[Vector2i(int(entry["loc"][0]), int(entry["loc"][1]))] = entry["hash"]
	return out

## With Godot closed the CLI changes the region size and vertex spacing of a folder for good, after a backup, and
## leaves every other folder alone.
static func _test_cli_settings(t) -> void:
	var folder := WORK.path_join("cli_settings")
	(await _world(t, "cli_settings")).free()
	(await _world(t, "cli_settings_other")).free()
	var other := _tree(WORK.path_join("cli_settings_other"))
	var first := _hashes(folder)
	var originals := _tree(folder)

	var bigger := Export._cli(["settings", folder, "128", "2.5"])
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	var files := _region_files(folder)
	t.check(bigger[0] == 0 and bigger[1].get("ok") == true and bigger[1].get("error") == "", "the CLI changes the region size to 128, got %s" % [bigger])
	t.check(files == { "terrain3d-01_00.res": [128, SPACING], "terrain3d_00-01.res": [128, SPACING], "terrain3d_01-01.res": [128, SPACING] }, "only the region files of size 128 are left, got %s" % [files])
	t.check(int(manifest.get("region_size", 0)) == 128 and float(manifest.get("scene_vertex_spacing", 0.0)) == SPACING and manifest.get("dirty") == [] and not manifest.has("unsaved_settings") and _hashes(folder).size() == 3, "the cache names size 128, no unsaved region and the spacing for the scene, got %s" % [manifest])
	var saved: String = bigger[1].get("backup", "")
	var copies := _tree(saved)
	var same := copies.keys().filter(func(k: String) -> bool: return copies[k] == originals.get(".trenchdot/manifest.json" if k == "manifest.json" else k))
	t.check(saved.begins_with(TrenchDotTerrain3DCache.cache_dir(folder).path_join("backups")) and copies.size() == 4 and same.size() == 4, "the backup holds the three region files and the manifest as they were, got %s in %s" % [copies.keys(), saved])

	var back := Export._cli(["settings", folder, "64", "2.5"])
	var hashes := _hashes(folder)
	var kept := first.keys().filter(func(loc: Vector2i) -> bool: return hashes.get(loc) == first[loc])
	files = _region_files(folder)
	t.check(back[0] == 0 and kept.size() == 3, "back at size 64 the regions have their heights, control and color again, got %s of %s" % [kept, first])
	t.check(files.size() == hashes.size() and files.values().all(func(f: Array) -> bool: return f == [SIZE, SPACING]), "every region file is of size 64 and the cache lists them all, got %s and %s" % [files, hashes.keys()])

	var spaced := Export._cli(["settings", folder, "64", "1.0"])
	manifest = TrenchDotTerrain3DCache.read_manifest(folder)
	var header := Export._decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(-1, 0))))
	t.check(spaced[0] == 0 and _hashes(folder) == hashes and float(manifest.get("vertex_spacing", 0.0)) == 1.0 and float(manifest.get("scene_vertex_spacing", 0.0)) == 1.0 and header.get("spacing") == 1.0, "a spacing change alone keeps every region and gives the cache the spacing, got %s and %s" % [spaced, header.get("spacing")])
	t.check(_region_files(folder).values().all(func(f: Array) -> bool: return f == [SIZE, 1.0]), "and every region file, got %s" % [_region_files(folder)])

	var again := Export._cli(["settings", folder, "64", "2.5"])
	var backups := Array(DirAccess.get_directories_at(TrenchDotTerrain3DCache.cache_dir(folder).path_join("backups")))
	t.check(again[0] == 0 and backups.size() == 3 and not DirAccess.dir_exists_absolute(saved) and DirAccess.dir_exists_absolute(again[1].get("backup", "")), "four changes keep the last three backups, got %s" % [backups])
	var exported := Export._cli(["export", folder])
	t.check(exported[0] == 0 and float(TrenchDotTerrain3DCache.read_manifest(folder).get("scene_vertex_spacing", 0.0)) == SPACING, "a CLI export keeps the spacing for the scene, got %s" % [exported])
	t.check(_tree(WORK.path_join("cli_settings_other")) == other, "the other data folder is left alone")

	var empty := WORK.path_join("cli_settings_empty")
	new_folder(empty, SIZE, 1.0)
	var none := Export._cli(["settings", empty, "256", "2.0"])
	manifest = TrenchDotTerrain3DCache.read_manifest(empty)
	t.check(none[0] == 0 and int(manifest.get("region_size", 0)) == 256 and float(manifest.get("vertex_spacing", 0.0)) == 2.0 and manifest.get("regions") == [] and _region_files(empty).is_empty(), "a folder without regions gets only a manifest with the new settings, got %s and %s" % [none, manifest])

## A change the folder cannot take is refused with nothing written, and so are wrong arguments.
static func _test_settings_refusals(t) -> void:
	var wide := WORK.path_join("settings_wide")
	var terrain: Node3D = await Export._terrain(t, wide, 128, 1.0)
	for loc in [Vector2i(-16, 0), Vector2i(15, 3)]:
		terrain.get(&"data").add_region_blank(loc, false)
	terrain.get(&"data").save_directory(wide)
	TrenchDotTerrain3DCache.export(terrain, wide)
	terrain.free()
	var untouched := _tree(wide)
	var refused := Export._cli(["settings", wide, "64", "1.0"])
	t.check(refused[0] == 1 and refused[1].get("ok") == false and str(refused[1].get("error")).contains("-16 to 15") and _tree(wide) == untouched, "a size that spreads the regions past -16 to 15 is refused with nothing written, got %s" % [refused])

	var crowded := WORK.path_join("settings_instances")
	terrain = await _world(t, "settings_instances")
	_add_instance(terrain)
	terrain.get(&"data").save_directory(crowded)
	TrenchDotTerrain3DCache.export(terrain, crowded)
	terrain.free()
	var before := _tree(crowded)
	refused = Export._cli(["settings", crowded, "128", "2.5"])
	t.check(refused[0] == 1 and str(refused[1].get("error")).contains("(-1, 0) hold Terrain3D instances") and _tree(crowded) == before, "a folder with instances is refused with nothing written, got %s" % [refused])

	var waiting := WORK.path_join("settings_pending")
	terrain = await _world(t, "settings_pending")
	_pending(waiting, Vector2i(-1, 0), { "height": _slanted(Vector2i(-1, 0), 2.0), "control": Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF) }, _hash(waiting, Vector2i(-1, 0)))
	before = _tree(waiting)
	var result := TrenchDotTerrain3DCache.rewrite_settings(terrain, waiting, 128, SPACING)
	t.check(not result.ok and result.error.contains("not applied") and _tree(waiting) == before and int(terrain.get(&"region_size")) == SIZE, "a folder with pending edits is refused with nothing written, got %s" % [result])
	terrain.free()

	var cli: GDScript = load("res://addons/func_godot/src/trenchdot/terrain3d_cli.gd")
	for args in [["settings", wide], ["settings", wide, "128"], ["settings", wide, "128", "2.5", "1"], ["export", wide, "128", "2.5"], ["settings", wide, "big", "2.5"], ["settings", wide, "128.5", "2.5"], ["settings", wide, "128", "wide"]]:
		t.check(cli.refusal(PackedStringArray(args)).begins_with("usage"), "the CLI refuses %s with its usage, got %s" % [args, cli.refusal(PackedStringArray(args))])
	for config in [[["settings", wide, "100", "2.5"], "no region size 100"], [["settings", wide, "128", "0.125"], "0.25 to 100"], [["settings", wide, "4096", "1"], "no region size 4096"], [["settings", wide, "128", "-1"], "0.25 to 100"]]:
		t.check(cli.refusal(PackedStringArray(config[0])).contains(config[1]), "the CLI refuses %s, got %s" % [config[0], cli.refusal(PackedStringArray(config[0]))])
	t.check(cli.refusal(PackedStringArray(["settings", wide, "2048", "100"])) == "", "it takes the largest size and spacing")
	var wrong := Export._cli(["settings", wide, "100", "2.5"])
	t.check(wrong[0] == 1 and wrong[1].get("ok") == false and str(wrong[1].get("error")).contains("no region size 100") and _tree(wide) == untouched, "a wrong size exits with 1 and names it, got %s" % [wrong])

## A region that cannot be saved at the new size puts the region files back from the backup and leaves the cache as
## it was.
static func _test_settings_save_fails(t) -> void:
	var folder := WORK.path_join("settings_blocked")
	(await _world(t, "settings_blocked")).free()
	# (3, -2) at size 64 lands at (1, -1) at size 128, a folder of that file name keeps it from being saved.
	DirAccess.make_dir_absolute(folder.path_join(TrenchDotTerrain3DCache.region_stem(Vector2i(1, -1)) + ".res"))
	var before := _tree(folder)
	var failed := Export._cli(["settings", folder, "128", "2.5"])
	var error := str(failed[1].get("error"))
	t.check(failed[0] == 1 and failed[1].get("ok") == false and error.contains("(1, -1)") and error.contains("put back") and DirAccess.dir_exists_absolute(failed[1].get("backup", "")), "a region that fails to save is named and the files are put back, got %s" % [failed])
	var after := _tree(folder)
	for path: String in after.keys():
		if path.begins_with(".trenchdot/backups"):
			after.erase(path)
	t.check(after == before, "the region files and the cache are as they were, got %s for %s" % [after.keys(), before.keys()])

## A region file Terrain3D could not load is not one the settings change merged away, so it stays as it was.
static func _test_settings_keep_unloaded(t) -> void:
	var folder := WORK.path_join("settings_unloaded")
	(await _world(t, "settings_unloaded")).free()
	var broken := folder.path_join(TrenchDotTerrain3DCache.region_stem(Vector2i(5, 5)) + ".res")
	FileAccess.open(broken, FileAccess.WRITE).store_string("not a region")
	var md5 := FileAccess.get_md5(broken)
	var changed := Export._cli(["settings", folder, "128", "2.5"])
	var problems: Array = changed[1].get("problems", [])
	t.check(changed[0] == 0 and changed[1].get("ok") == true and FileAccess.get_md5(broken) == md5, "a region file that did not load stays, got %s" % [changed])
	t.check(problems.any(func(p: String) -> bool: return p.contains(broken.get_file()) and p.contains("did not load")) and not changed[1].get("removed", []).has(broken.get_file()), "and is named as a problem, got %s" % [problems])
	t.check(changed[1].get("removed", []).has(TrenchDotTerrain3DCache.region_stem(Vector2i(3, -2)) + ".res"), "while the file of a loaded region that merged away goes, got %s" % [changed[1].get("removed")])

## A scene's Terrain3D on [param folder] at the spacing the test worlds have, in a slot under a map node.
static func _reopened(t, folder: String) -> TrenchDotTerrain3D:
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"vertex_spacing", SPACING)
	terrain.set(&"data_directory", folder)
	t.root.add_child(terrain)
	await t.process_frame
	return await _slot(t, terrain)

## The spacing a change made with Godot closed leaves for the scene reaches the scene's Terrain3D when the map is built
## over it or the scene opens in the editor, and exports name it until the scene is saved.
static func _test_scene_spacing(t) -> void:
	var folder := WORK.path_join("scene_spacing")
	(await _world(t, "scene_spacing")).free()
	var changed := Export._cli(["settings", folder, str(SIZE), "1.0"])
	var hashes := _hashes(folder)
	var slot := await _reopened(t, folder)
	var terrain := slot.get_terrain()
	t.check(changed[0] == 0 and is_equal_approx(float(terrain.get(&"vertex_spacing")), SPACING), "a game leaves the spacing of the scene alone, got %s" % [terrain.get(&"vertex_spacing")])
	var report := TrenchDotBuildReport.new("")
	TrenchDotTerrain3D.build(slot.get_parent(), [{ "data": { "data_directory": folder } }], report)
	t.check(is_equal_approx(float(terrain.get(&"vertex_spacing")), 1.0) and report.terrain3d_problems.is_empty(), "a build over the scene's Terrain3D gives it the spacing, got %s and %s" % [terrain.get(&"vertex_spacing"), report.terrain3d_problems])
	t.check(is_equal_approx(float(TrenchDotTerrain3DCache.read_manifest(folder).get("scene_vertex_spacing", 0.0)), 1.0), "and a game leaves the cache as it was")
	slot.get_parent().free()

	TrenchDotTerrain3D.export_outside_editor = true
	slot = await _reopened(t, folder)
	terrain = slot.get_terrain()
	await t.process_frame
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(is_equal_approx(float(terrain.get(&"vertex_spacing")), 1.0) and _hashes(folder) == hashes, "an opened scene takes the spacing and keeps every region, got %s" % [terrain.get(&"vertex_spacing")])
	t.check(is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), 1.0) and is_equal_approx(float(manifest.get("scene_vertex_spacing", 0.0)), 1.0), "its exports name the spacing for the scene until it is saved, got %s and %s" % [manifest.get("vertex_spacing"), manifest.get("scene_vertex_spacing")])
	slot.notification(Node.NOTIFICATION_EDITOR_POST_SAVE)
	manifest = TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(not manifest.has("scene_vertex_spacing") and is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), 1.0), "the export after a save drops it, got %s" % [manifest])
	TrenchDotTerrain3D.export_outside_editor = false
	terrain.set(&"vertex_spacing", 3.0)
	TrenchDotTerrain3D.build(slot.get_parent(), [{ "data": { "data_directory": folder } }], report)
	t.check(is_equal_approx(float(terrain.get(&"vertex_spacing")), 3.0), "without it a build keeps the spacing the scene has")
	slot.get_parent().free()

## What TrenchDot writes when it links a folder with no regions yet: a manifest with the size and spacing the new
## terrain should get, and no region file.
static func new_folder(folder: String, region_size: int, spacing: float) -> void:
	Export._clear(folder)
	DirAccess.make_dir_recursive_absolute(TrenchDotTerrain3DCache.cache_dir(folder))
	var manifest := { "format": TrenchDotTerrain3DCache.FORMAT, "version": TrenchDotTerrain3DCache.VERSION, "region_size": region_size, "vertex_spacing": spacing, "regions": [] }
	FileAccess.open(TrenchDotTerrain3DCache.cache_dir(folder).path_join(TrenchDotTerrain3DCache.MANIFEST), FileAccess.WRITE).store_string(JSON.stringify(manifest))

static func _test_new_folder_settings(t) -> void:
	for config in [[128, 2.5], [512, 0.75]]:
		var size: int = config[0]
		var spacing: float = config[1]
		var folder := WORK.path_join("new_%d" % size)
		new_folder(folder, size, spacing)
		var loc := Vector2i(-3, 2)
		var heights := PackedByteArray()
		heights.resize(size * size * 4)
		for i in size * size:
			heights.encode_float(i * 4, 7.0 + (i % size) * 0.125 - (i / size) * 0.0625)
		var control := PackedByteArray()
		control.resize(size * size * 4)
		var maps := { "height": Image.create_from_data(size, size, false, Image.FORMAT_RF, heights), "control": Image.create_from_data(size, size, false, Image.FORMAT_RF, control) }
		var path := TrenchDotTerrain3DCache.cache_dir(folder).path_join("pending").path_join(TrenchDotTerrain3DCache.region_stem(loc) + TrenchDotTerrain3DCache.REGION_EXT)
		DirAccess.make_dir_recursive_absolute(path.get_base_dir())
		FileAccess.open(path, FileAccess.WRITE).store_buffer(_pending_bytes(loc, size, spacing, maps, NO_BASE))
		var applied := Export._cli(["apply", folder])
		t.check(applied[0] == 0 and Export._ints(applied[1].get("accepted", [])) == [[-3, 2]], "the CLI applies a region to a new folder of size %d, got %s" % [size, applied])
		var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
		t.check(int(manifest.get("region_size", 0)) == size and t.near(float(manifest.get("vertex_spacing", 0.0)), spacing), "the cache of the new folder keeps size %d and spacing %s, got %s" % [size, spacing, [manifest.get("region_size"), manifest.get("vertex_spacing")]])
		var saved := _saved_height(folder, loc)
		t.check(saved != null and saved.get_width() == size and saved.get_data() == heights, "the region file of the new folder is %d wide and holds the heights" % size)
		var region: Resource = ResourceLoader.load(folder.path_join("terrain3d-03_02.res"), "", ResourceLoader.CACHE_MODE_IGNORE)
		t.check(region != null and int(region.get(&"region_size")) == size, "Terrain3D saved the region at size %d" % size)

static func _test_parse_region_file(t) -> void:
	var cases := {
		"terrain3d-01_02.res": Vector2i(-1, 2), "terrain3d_15-16.res": Vector2i(15, -16), "terrain3d_00_00.res": Vector2i(0, 0),
		"terrain3d_00_00.res.import": null, "terrain3d-1_02.res": null, "terrain3d_00_00.td3r": null, "xterrain3d_00_00.res": null,
	}
	for file in cases:
		t.check(TrenchDotTerrain3DCache.parse_region_file(file) == cases[file], "%s names region %s" % [file, cases[file]])

## A slot holding [param terrain] under a map node in the tree, ready once it looked at the region files.
static func _slot(t, terrain: Node3D) -> TrenchDotTerrain3D:
	var map := Node3D.new()
	var slot := TrenchDotTerrain3D.new()
	terrain.get_parent().remove_child(terrain)
	slot.add_child(terrain)
	map.add_child(slot)
	t.root.add_child(map)
	await t.process_frame
	return slot

## Rewrites the region file of [param loc] with heights lifted by [param lift] the way another Godot would, a second
## later so its modified time differs.
static func _rewrite(t, folder: String, loc: Vector2i, lift: float) -> void:
	await t.create_timer(1.1).timeout
	var path := folder.path_join(TrenchDotTerrain3DCache.region_stem(loc) + ".res")
	var region: Resource = ResourceLoader.load(path, "", ResourceLoader.CACHE_MODE_IGNORE)
	region.call(&"set_map", TYPE_HEIGHT, _slanted(loc, lift))
	ResourceSaver.save(region, path, ResourceSaver.FLAG_COMPRESS)

static func _test_reload_in_game(t) -> void:
	var folder := WORK.path_join("reload")
	var terrain := await _world(t, "reload")
	var slot := await _slot(t, terrain)
	var data: Object = terrain.get(&"data")
	var west := Vector2i(-1, 0)
	var north := Vector2i(0, -1)
	var south := Vector2i(3, -2)
	var cached := _hash(folder, west)
	t.check(slot.reload_changed() == [], "nothing reloads while the files are unchanged")
	await _rewrite(t, folder, west, 11.0)
	data.get_region(north).get_map(TYPE_HEIGHT).set_pixel(1, 2, Color(-9.0, 0, 0))
	data.get_region(north).set(&"modified", true)
	await _rewrite(t, folder, north, 4.0)
	DirAccess.remove_absolute(folder.path_join(TrenchDotTerrain3DCache.region_stem(south) + ".res"))
	DirAccess.copy_absolute(folder.path_join("terrain3d-01_00.res"), folder.path_join("terrain3d_02_01.res"))
	var report := TrenchDotBuildReport.new("")
	var built := TrenchDotTerrain3D.build(slot.get_parent(), [{ "data": { "data_directory": folder } }], report)
	t.check(built == slot and report.terrain3d_problems.is_empty(), "a rebuild of the map in a game keeps the slot, got %s" % [report.terrain3d_problems])
	t.check(data.get_region(west).get_map(TYPE_HEIGHT).get_data() == _slanted(west, 11.0).get_data(), "the rebuild reloads a region whose file changed")
	t.check(t.near(data.get_region(north).get_map(TYPE_HEIGHT).get_pixel(1, 2).r, -9.0) and data.get_region(north).get(&"modified"), "a region with unsaved edits keeps them")
	t.check(not data.has_region(south) and data.has_region(Vector2i(2, 1)) and data.get_region(Vector2i(2, 1)).get_map(TYPE_HEIGHT).get_data() == _slanted(west, 11.0).get_data(), "a removed file removes its region and a new file adds one")
	t.check(t.near(data.get_height(Vector3(-SIZE * SPACING + 2 * SPACING, 0, 6 * SPACING)), 11.0 - 3.0 + 2 * 0.75 - 6 * 0.3125), "Terrain3D samples the reloaded height")
	t.check(_hash(folder, west) == cached, "a game does not write the cache")
	t.check(slot.reload_changed() == [], "a second look finds nothing new")
	slot.get_parent().free()

static func _test_editor_watch(t) -> void:
	var folder := WORK.path_join("watch")
	var terrain := await _world(t, "watch")
	var west := Vector2i(-1, 0)
	var south := Vector2i(3, -2)
	_pending(folder, south, { "height": _slanted(south, 5.0), "control": Export._pattern(south, false)["control"] }, _hash(folder, south))
	TrenchDotTerrain3D.export_outside_editor = true
	var slot := await _slot(t, terrain)
	await t.process_frame
	var data: Object = terrain.get(&"data")
	t.check(data.get_region(south).get_map(TYPE_HEIGHT).get_data() == _slanted(south, 5.0).get_data() and _pending_left(folder).is_empty(), "the slot applies pending edits when it enters the scene")
	var seen: Array = []
	slot.regions_reloaded.connect(func(locs: Array): seen.append_array(locs))
	await _rewrite(t, folder, west, -6.0)
	await t.create_timer(TrenchDotTerrain3D.WATCH_INTERVAL + 0.2).timeout
	t.check(seen == [west] and data.get_region(west).get_map(TYPE_HEIGHT).get_data() == _slanted(west, -6.0).get_data(), "the editor watch reloads a region changed on disk within a second, got %s" % [seen])
	t.check(_hash(folder, west) == TrenchDotTerrain3DCache.region_hash(data.get_region(west), west), "and exports it to the cache")
	await t.create_timer(TrenchDotTerrain3D.WATCH_INTERVAL + 0.2).timeout
	t.check(seen == [west], "its own apply and export trigger no reload, got %s" % [seen])
	TrenchDotTerrain3D.export_outside_editor = false
	slot.get_parent().free()

## A region file changed while no editor had the scene open reaches the cache once the scene opens, and a region the
## cache holds another hash for counts as stale.
static func _test_changed_while_closed(t) -> void:
	var folder := WORK.path_join("closed")
	var west := Vector2i(-1, 0)
	var north := Vector2i(0, -1)
	(await _world(t, "closed")).free()
	var cached := _hash(folder, west)
	var untouched := _hash(folder, north)
	await _rewrite(t, folder, west, 7.0)
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"vertex_spacing", SPACING)
	terrain.set(&"data_directory", folder)
	t.root.add_child(terrain)
	await t.process_frame
	TrenchDotTerrain3D.export_outside_editor = true
	var slot := await _slot(t, terrain)
	await t.process_frame
	var data: Object = terrain.get(&"data")
	var now := TrenchDotTerrain3DCache.region_hash(data.get_region(west), west)
	t.check(data.get_region(west).get_map(TYPE_HEIGHT).get_data() == _slanted(west, 7.0).get_data(), "the reopened scene loads the region changed on disk")
	t.check(_hash(folder, west) != cached and _hash(folder, west) == now and _hash(folder, north) == untouched, "and exports it to the cache when it opens, got %s" % [[cached, _hash(folder, west), now]])
	t.check(Export._decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, west))).get("hash") == now, "the region file in the cache carries the new hash")
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	for entry in manifest["regions"]:
		if Vector2i(int(entry["loc"][0]), int(entry["loc"][1])) == north:
			entry["hash"] = NO_BASE
	FileAccess.open(TrenchDotTerrain3DCache.cache_dir(folder).path_join(TrenchDotTerrain3DCache.MANIFEST), FileAccess.WRITE).store_string(JSON.stringify(manifest))
	t.check(slot._stale_regions() == [north], "a region whose cached hash differs is stale, got %s" % [slot._stale_regions()])
	TrenchDotTerrain3D.export_outside_editor = false
	slot.get_parent().free()

## Godot closed without saving a settings change the live link had made, so the cache names a region size and spacing
## the region files do not have. The cache has to follow the files again.
static func _lost_settings(t, name: String, size_factor: int, spacing_factor: float) -> String:
	var terrain := await _world(t, name)
	TrenchDotTerrain3D.export_outside_editor = true
	var slot := await _slot(t, terrain)
	slot.change_settings(SIZE * size_factor, SPACING * spacing_factor)
	slot.get_parent().free()
	TrenchDotTerrain3D.export_outside_editor = false
	return WORK.path_join(name)

## Every region of the manifest has a cache file of the saved size and spacing with the hash the manifest names, and
## the manifest lists the regions on disk and nothing else.
static func _follows_files(t, folder: String, label: String) -> void:
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(int(manifest.get("region_size", 0)) == SIZE and is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), SPACING), "%s: the manifest holds region size %d and spacing %s, got %s and %s" % [label, SIZE, SPACING, manifest.get("region_size"), manifest.get("vertex_spacing")])
	var listed := []
	var wrong := []
	for entry in manifest.get("regions", []):
		var loc := Vector2i(int(entry["loc"][0]), int(entry["loc"][1]))
		listed.append([loc.x, loc.y])
		var file := Export._decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, loc)))
		if file.get("size") != SIZE or not is_equal_approx(file.get("spacing", 0.0), SPACING) or file.get("hash") != entry["hash"]:
			wrong.append([loc, file.get("size"), file.get("spacing")])
	listed.sort()
	t.check(listed == [[-1, 0], [0, -1], [3, -2]] and wrong.is_empty(), "%s: the cache files of the regions on disk agree with the manifest, got %s and %s" % [label, listed, wrong])
	t.check(not manifest.has("unsaved_settings"), "%s: the manifest no longer names a settings change, got %s" % [label, manifest.get("unsaved_settings")])

static func _test_lost_settings(t) -> void:
	var west := Vector2i(-1, 0)
	# Region size and spacing lost, the edit TrenchDot made at the lost size is refused and the cache heals.
	var folder := await _lost_settings(t, "lost_both", 2, 2.0)
	var lost := TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(int(lost["region_size"]) == SIZE * 2 and is_equal_approx(float(lost["vertex_spacing"]), SPACING * 2.0), "the live link settings reached the cache and not the files")
	var big := { "height": Image.create_empty(SIZE * 2, SIZE * 2, false, Image.FORMAT_RF), "control": Image.create_empty(SIZE * 2, SIZE * 2, false, Image.FORMAT_RF) }
	_pending(folder, west, big, NO_BASE, FORCE, SIZE * 2)
	var applied := Export._cli(["apply", folder])
	t.check(applied[0] == 0 and applied[1].get("accepted") == [] and Export._ints(applied[1].get("refused", []).map(func(r: Dictionary): return [r["loc"], r["reason"]])) == [[[-1, 0], "size"]], "the edit at the lost region size is refused as size, got %s" % [applied])
	_follows_files(t, folder, "after a refused apply")
	# Only the spacing lost, an edit at the real size is applied and the cache keeps the files' spacing.
	folder = await _lost_settings(t, "lost_spacing", 1, 2.0)
	var small := { "height": _slanted(west, 11.0), "control": Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RF) }
	_pending(folder, west, small, _hash(folder, west))
	applied = Export._cli(["apply", folder])
	t.check(applied[0] == 0 and Export._ints(applied[1].get("accepted")) == [[-1, 0]] and applied[1].get("refused") == [], "the edit at the real region size is applied, got %s" % [applied])
	_follows_files(t, folder, "after an accepted apply")
	t.check(_saved_height(folder, west).get_data() == small["height"].get_data(), "and the region file holds it")
	# A bare export without pending edits.
	folder = await _lost_settings(t, "lost_export", 2, 2.0)
	var exported := Export._cli(["export", folder])
	t.check(exported[0] == 0, "the CLI exports a folder with a stale cache, got %s" % [exported])
	_follows_files(t, folder, "after a CLI export")
	# A saved scene opened again, the node has the files' spacing and the hashes of the regions did not change.
	for config in [[1, 2.0], [2, 2.0]]:
		folder = await _lost_settings(t, "lost_reopen_%d" % config[0], config[0], config[1])
		var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
		terrain.set(&"vertex_spacing", SPACING)
		terrain.set(&"data_directory", folder)
		t.root.add_child(terrain)
		await t.process_frame
		TrenchDotTerrain3D.export_outside_editor = true
		var slot := await _slot(t, terrain)
		await t.process_frame
		_follows_files(t, folder, "after reopening with size factor %d" % config[0])
		t.check(slot._stale_regions() == [], "nothing is stale once the cache follows the files, got %s" % [slot._stale_regions()])
		var before := FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.cache_dir(folder).path_join(TrenchDotTerrain3DCache.MANIFEST))
		slot._export_stale()
		t.check(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.cache_dir(folder).path_join(TrenchDotTerrain3DCache.MANIFEST)) == before, "and opening it again leaves the manifest as it is")
		TrenchDotTerrain3D.export_outside_editor = false
		slot.get_parent().free()

static func _with_manifest(folder: String, spacing: float, dirty: Array, unsaved: Variant) -> void:
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	manifest["vertex_spacing"] = spacing
	manifest["dirty"] = dirty
	manifest.erase("unsaved_settings")
	if unsaved != null:
		manifest["unsaved_settings"] = unsaved
	FileAccess.open(TrenchDotTerrain3DCache.cache_dir(folder).path_join(TrenchDotTerrain3DCache.MANIFEST), FileAccess.WRITE).store_string(JSON.stringify(manifest))

## A Terrain3D draws with the spacing of its scene node, which a node made by hand on an old folder need not share with
## the region files. The CLI keeps the manifest's spacing then, also over unsaved sculpting Godot lost, and turns to the
## region file saved last only when the manifest holds a settings change Godot had not saved.
static func _test_spacing_sources(t) -> void:
	var folder := WORK.path_join("spacing_sources")
	var terrain := await _world(t, "spacing_sources")
	terrain.free()
	t.check(is_equal_approx(TrenchDotTerrain3DCache.region_spacing(folder), SPACING), "three region files saved at one spacing give it, got %s" % TrenchDotTerrain3DCache.region_spacing(folder))
	await t.create_timer(1.1).timeout
	var by_hand := ClassDB.instantiate(&"Terrain3D") as Node3D
	by_hand.set(&"vertex_spacing", 1.0)
	by_hand.set(&"data_directory", folder)
	t.root.add_child(by_hand)
	await t.process_frame
	Export._fill(by_hand, Vector2i(-2, 1), Export._pattern(Vector2i(-2, 1), false))
	by_hand.get(&"data").save_directory(folder)
	var old: Resource = ResourceLoader.load(folder.path_join("terrain3d-01_00.res"), "", ResourceLoader.CACHE_MODE_IGNORE)
	t.check(is_equal_approx(float(old.get(&"vertex_spacing")), SPACING) and is_equal_approx(TrenchDotTerrain3DCache.region_spacing(folder), 1.0), "the region file saved last wins over three older ones that sort around it, got %s and %s" % [old.get(&"vertex_spacing"), TrenchDotTerrain3DCache.region_spacing(folder)])
	TrenchDotTerrain3DCache.export(by_hand, folder, null, { "editor_open": true })
	by_hand.free()
	var exported := Export._cli(["export", folder])
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(exported[0] == 0 and exported[1].get("written") == [] and is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), 1.0), "the CLI keeps the spacing the editor exported and rewrites nothing, got %s and %s" % [exported, manifest.get("vertex_spacing")])
	var all := [[3, -2], [-1, 0], [0, -1], [-2, 1]]
	var marker := { "region_size": SIZE * 2, "vertex_spacing": 2.0 }
	for config in [[[], null, 2.0], [[[3, -2]], null, 2.0], [all, null, 2.0], [[[3, -2]], marker, 1.0], [all, marker, 1.0]]:
		_with_manifest(folder, 2.0, config[0], config[1])
		exported = Export._cli(["export", folder])
		manifest = TrenchDotTerrain3DCache.read_manifest(folder)
		var header := Export._decode(FileAccess.get_file_as_bytes(TrenchDotTerrain3DCache.region_path(folder, Vector2i(3, -2))))
		t.check(exported[0] == 0 and is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), config[2]) and is_equal_approx(header.get("spacing", 0.0), config[2]) and manifest.get("dirty") == [] and not manifest.has("unsaved_settings"), "with %d unsaved regions and %s settings change the CLI exports at spacing %s, got %s and %s" % [config[0].size(), "a" if config[1] else "no", config[2], manifest.get("vertex_spacing"), header.get("spacing")])

## The manifest names the settings a live link change replaced, which the scene was saved with, until Godot saves the
## regions again. A second change keeps the first ones.
static func _test_unsaved_settings(t) -> void:
	var folder := WORK.path_join("unsaved_settings")
	var terrain := await _world(t, "unsaved_settings")
	TrenchDotTerrain3D.export_outside_editor = true
	var slot := await _slot(t, terrain)
	var marker := func() -> Variant: return TrenchDotTerrain3DCache.read_manifest(folder).get("unsaved_settings")
	slot.export_cache(null, false)
	t.check(marker.call() == null, "a manifest without a settings change names none, got %s" % [marker.call()])
	var saved := func(m: Variant) -> bool: return m is Dictionary and int(m.get("region_size", 0)) == SIZE and is_equal_approx(float(m.get("vertex_spacing", 0.0)), SPACING)
	slot.change_settings(SIZE * 2, SPACING)
	t.check(saved.call(marker.call()), "a region size change names the size and spacing it replaced, got %s" % [marker.call()])
	slot.change_settings(SIZE * 2, SPACING * 3.0)
	t.check(saved.call(marker.call()), "a second change keeps the settings the scene was saved with, got %s" % [marker.call()])
	slot.export_cache([], false)
	var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(saved.call(manifest.get("unsaved_settings")) and not manifest["dirty"].is_empty(), "an export while regions are unsaved keeps it, got %s and %s" % [manifest.get("unsaved_settings"), manifest["dirty"]])
	terrain.get(&"data").save_directory(folder)
	slot.export_cache([], false)
	manifest = TrenchDotTerrain3DCache.read_manifest(folder)
	t.check(manifest.get("dirty") == [] and not manifest.has("unsaved_settings"), "the export after a save drops it, got %s and %s" % [manifest.get("dirty"), manifest.get("unsaved_settings")])
	TrenchDotTerrain3D.export_outside_editor = false
	slot.get_parent().free()

## A region size that would spread the regions past the locations Terrain3D keeps is refused and leaves the terrain as
## it was, one that keeps them inside is taken.
static func _test_settings_overflow(t) -> void:
	var folder := WORK.path_join("overflow")
	Export._clear(folder)
	DirAccess.make_dir_recursive_absolute(folder)
	var slot := TrenchDotTerrain3D.new()
	t.root.add_child(slot)
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"data_directory", folder)
	slot.add_child(terrain)
	await t.process_frame
	terrain.set(&"region_size", 128)
	var data: Object = terrain.get(&"data")
	for loc in [Vector2i(-16, 0), Vector2i(15, 3)]:
		data.add_region_blank(loc, false)
	var refusal := slot.change_settings(64, 1.0)
	var locs := Array(data.get_region_locations())
	locs.sort()
	t.check(refusal.contains("-16 to 15") and refusal.contains("(-32, 0)") and refusal.contains("(31, 7)"), "a region size that spreads the regions past -16 to 15 is refused, got %s" % refusal)
	t.check(int(terrain.get(&"region_size")) == 128 and locs == [Vector2i(-16, 0), Vector2i(15, 3)], "and the terrain stays as it was, got %d %s" % [terrain.get(&"region_size"), locs])
	for loc in [Vector2i(-16, 0), Vector2i(15, 3)]:
		data.remove_regionl(loc, false)
	for loc in [Vector2i(-8, 0), Vector2i(7, 3)]:
		data.add_region_blank(loc, false)
	refusal = slot.change_settings(64, 1.0)
	locs = Array(data.get_region_locations())
	t.check(refusal == "" and int(terrain.get(&"region_size")) == 64 and locs.all(func(l: Vector2i) -> bool: return l.x >= -16 and l.x <= 15), "a size that keeps them inside is taken, got %s %d %s" % [refusal, terrain.get(&"region_size"), locs])
	slot.free()
	Export._clear(folder)
