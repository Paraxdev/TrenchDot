extends RefCounted
## The Terrain3D link: version 2 maps parse, the build gives the map a TrenchDotTerrain3D slot holding a Terrain3D on
## the linked folder, keeps a Terrain3D the user put there, survives every kind of rebuild, and reports a missing
## Terrain3D, a folder mismatch and a map away from the origin. Terrain3D is fetched by tools/fetch_terrain3d.py.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const MAP := "res://tests/maps/basic.tdm"
const MAP_JSON := "res://tests/maps/basic_json.tdm"
const SETTINGS := "res://demo/demo_map_settings.tres"
## One blank region at (-1, 2) written by Terrain3D 1.1.0, with a height of 12.5 m set at SPOT, which Terrain3D stored
## on the vertex VERTEX.
const DATA := "res://tests/terrain3d/probe"
const SPOT := Vector2(-100.5, 600.25)
const VERTEX := Vector2(-101, 600)
const LINK_ID := 40
const Apply := preload("res://tests/terrain3d_apply_tests.gd")

static func run(t) -> void:
	print("- Terrain3D link")
	_test_parser(t)
	_test_binary(t)
	_test_streamer_leaves_slot_alone(t)
	_test_scripts_name_no_terrain3d_class(t)
	await _test_without_terrain3d(t)
	t.check(ClassDB.class_exists("Terrain3D"), "Terrain3D is installed, fetch it with python3 tools/fetch_terrain3d.py")
	if not ClassDB.class_exists("Terrain3D"):
		return
	await _test_builds_slot(t)
	await _test_new_folder_size(t)
	await _test_keeps_user_terrain(t)
	await _test_problems(t)
	await _test_live_session_and_hot_reload(t)
	await _test_height_at(t)

## basic_json.tdm as [param version] with a terrain3d node per folder in [param folders] in its first layer.
static func _map_text(folders: Array, version := 2) -> String:
	var json: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(MAP_JSON))
	json["version"] = version
	for i in folders.size():
		json["layers"][0]["children"].append({ "id": LINK_ID + i, "type": "terrain3d", "label": "Terrain3D", "data_directory": folders[i] })
	return JSON.stringify(json)

static func _level(t, text := "") -> FuncGodotMap:
	var level := Node3D.new()
	level.name = "Level"
	t.root.add_child(level)
	var map := FuncGodotMap.new()
	map.name = "Map"
	map.map_settings = load(SETTINGS)
	map.local_map_file = MAP
	level.add_child(map)
	map.owner = level
	if text == "":
		map.build()
	else:
		map.build_from_text(text)
	return map

static func _terrain3d_warnings(report: Dictionary) -> Array:
	return Array(report.get("warnings", [])).filter(func(w: String) -> bool: return w.contains("Terrain3D"))

static func _new_terrain(folder: String) -> Node3D:
	var terrain: Node3D = ClassDB.instantiate("Terrain3D")
	terrain.name = "MyTerrain"
	terrain.set("data_directory", folder)
	return terrain

static func _parse(json: Variant) -> FuncGodotData.ParseData:
	return TrenchDotParser.parse_dict(json, load(SETTINGS), FuncGodotData.ParseData.new(), "terrain3d.tdm")

static func _test_parser(t) -> void:
	t.check(TrenchDotParser.FORMAT_VERSION == 2, "the addon reads map version 2")
	var data := _parse(JSON.parse_string(_map_text([DATA])))
	t.check(data != null and data.terrain3d.size() == 1 and data.terrain3d[0]["id"] == LINK_ID and data.terrain3d[0]["data"]["data_directory"] == DATA, "a version 2 map collects its terrain3d node")
	t.check(data != null and data.entities.size() == _parse(TrenchDotTdmFile.load_map(MAP)).entities.size(), "the link adds no entity")
	var plain := _parse(TrenchDotTdmFile.load_map(MAP))
	t.check(plain != null and plain.terrain3d.is_empty(), "a version 1 map has no Terrain3D link")
	t.check(_parse(JSON.parse_string(_map_text([DATA], 3))) == null, "map version 3 is refused")
	var two := _parse(JSON.parse_string(_map_text(["res://a", "res://b"])))
	t.check(two.terrain3d.map(func(l): return l["data"]["data_directory"]) == ["res://a", "res://b"], "every terrain3d node is collected in map order")
	for ns in [0, 1]:
		var ctx := TrenchDotParser.Context.new()
		ctx.parse_data = FuncGodotData.ParseData.new()
		ctx.instance_ns = ns
		TrenchDotParser._parse_node(ctx, { "id": 7, "type": "terrain3d", "data_directory": DATA }, null)
		t.check(ctx.parse_data.terrain3d.size() == (1 if ns == 0 else 0), "a terrain3d node %s" % ("of the map is collected" if ns == 0 else "inside a prefab instance is left out"))

static func _test_binary(t) -> void:
	var head := var_to_bytes({ "format": "trenchdot-map", "version": 2 })
	var link := { "id": 9, "type": "terrain3d", "label": "Terrain3D", "data_directory": DATA }
	var node := var_to_bytes({ "nodes": [{ "id": 3, "type": "layer", "name": "Ground" }, link], "parents": PackedInt64Array([0, 3]) })
	var read := TrenchDotTdmFile.decode(load("res://tests/tdm_file_tests.gd")._container([["HEAD", head, 0, head.size()], ["NODE", node, 0, node.size()]]))
	var map: Variant = read["map"]
	t.check(map is Dictionary and map["version"] == 2 and map["layers"][0]["children"] == [link], "a binary version 2 map reads its terrain3d node, got %s" % [map])
	var data := _parse(map)
	t.check(data != null and data.terrain3d.size() == 1 and data.terrain3d[0]["id"] == 9, "a binary version 2 map collects its link")

static func _test_builds_slot(t) -> void:
	var bare := _level(t)
	t.check(TrenchDotTerrain3D.find(bare) == null, "a map without a link gets no slot")
	bare.owner.free()

	var text := _map_text([DATA + "/"])
	var map := _level(t, text)
	var level := map.owner
	var kept := Node3D.new()
	kept.name = "KeptMarker"
	kept.add_to_group(TrenchDotOverlay.KEEP_GROUP, true)
	map.add_child(kept)
	kept.owner = level
	var slot := map.get_child(0) as TrenchDotTerrain3D
	t.check(slot != null and slot.name == TrenchDotTerrain3D.NODE_NAME and slot.owner == level and slot.is_in_group(TrenchDotOverlay.KEEP_GROUP), "a map with a link gets a kept slot as its first child")
	var terrain := slot.get_terrain() if slot else null
	t.check(terrain != null and terrain.is_class("Terrain3D") and terrain.name == "Terrain3D" and terrain.owner == level and slot.get_child_count() == 1, "the slot holds one Terrain3D")
	t.check(terrain != null and terrain.get("data_directory") == DATA, "the Terrain3D uses the linked folder without its trailing slash, got %s" % [terrain.get("data_directory") if terrain else null])

	var report := {}
	for i in 3:
		report = map.build_from_text(text)
		await t.process_frame
	t.check(_terrain3d_warnings(report).is_empty(), "a linked map at the origin builds without Terrain3D warnings, got %s" % [report.get("warnings")])
	t.check(is_instance_valid(slot) and is_instance_valid(terrain) and slot.get_terrain() == terrain, "three builds keep the same slot and Terrain3D")
	t.check(t.collect(map, func(n): return n is TrenchDotTerrain3D).size() == 1 and t.collect(map, func(n): return n.is_class("Terrain3D")).size() == 1, "builds do not add a second slot or Terrain3D")
	t.check(map.get_child(0) == slot and map.get_child(1) == kept and String(map.get_child(2).name) == "entity_0_worldspawn", "the slot stays first, before the generated nodes, got %s" % [map.get_children().map(func(n): return String(n.name))])

	map.build()
	t.check(is_instance_valid(slot) and slot.get_terrain() == terrain, "building the map without its link keeps the slot")
	map.clear_children()
	t.check(is_instance_valid(slot) and is_instance_valid(terrain) and map.get_child_count() == 2, "Clear Map keeps the slot")

	map.build_from_text(text)
	var packed := PackedScene.new()
	t.check(packed.pack(level) == OK, "a level with a slot packs")
	var copy := packed.instantiate()
	var copy_slot := copy.get_node_or_null("Map/" + TrenchDotTerrain3D.NODE_NAME) as TrenchDotTerrain3D
	t.check(copy_slot != null and copy_slot.is_in_group(TrenchDotOverlay.KEEP_GROUP) and copy_slot.get_node_or_null("Terrain3D") != null, "the saved scene holds the slot, its group and its Terrain3D")
	copy.free()
	level.free()

## A folder with no regions gets a Terrain3D of the region size and vertex spacing its manifest names, a folder with
## regions keeps the size of its files.
static func _test_new_folder_size(t) -> void:
	for config in [[128, 2.5], [1024, 0.25]]:
		var folder := "user://td_t3d_new_size_%d" % config[0]
		Apply.new_folder(folder, config[0], config[1])
		var map := _level(t, _map_text([folder]))
		await t.process_frame
		var terrain := (map.get_child(0) as TrenchDotTerrain3D).get_terrain()
		t.check(terrain != null and int(terrain.get("region_size")) == config[0] and t.near(float(terrain.get("vertex_spacing")), config[1]), "a new folder builds a Terrain3D of size %d and spacing %s, got %s" % [config[0], config[1], [terrain.get("region_size"), terrain.get("vertex_spacing")] if terrain else null])
		map.owner.free()
		Apply.Export._clear(folder)

	var folder := "user://td_t3d_new_size_files"
	Apply.new_folder(folder, 128, 2.5)
	DirAccess.copy_absolute(DATA + "/terrain3d-01_02.res", folder + "/terrain3d-01_02.res")
	var probe := _level(t, _map_text([folder]))
	await t.process_frame
	var kept := (probe.get_child(0) as TrenchDotTerrain3D).get_terrain()
	t.check(kept != null and int(kept.get("region_size")) == 256, "a folder with regions keeps the size of its files, got %s" % [kept.get("region_size") if kept else null])
	probe.owner.free()
	Apply.Export._clear(folder)

static func _test_keeps_user_terrain(t) -> void:
	var map := _level(t)
	var level := map.owner
	var slot := TrenchDotTerrain3D.new()
	slot.name = "MySlot"
	map.add_child(slot)
	slot.owner = level
	var mine := _new_terrain("res://tests/terrain3d/elsewhere")
	slot.add_child(mine)
	mine.owner = level
	var report := map.build_from_text(_map_text([DATA]))
	t.check(TrenchDotTerrain3D.find(map) == slot and map.get_child(0) == slot and slot.get_terrain() == mine and slot.get_child_count() == 1, "a slot added by hand keeps the user's Terrain3D")
	t.check(mine.get("data_directory") == "res://tests/terrain3d/elsewhere", "the build leaves the user's data folder alone")
	var warnings := _terrain3d_warnings(report)
	t.check(warnings.size() == 1 and warnings[0].contains("res://tests/terrain3d/elsewhere") and warnings[0].contains(DATA), "a folder mismatch names both folders, got %s" % [warnings])
	mine.set("data_directory", DATA + "/")
	report = map.build_from_text(_map_text([DATA]))
	t.check(_terrain3d_warnings(report).is_empty(), "the same folder with a trailing slash is no mismatch, got %s" % [report.get("warnings")])
	level.free()

static func _test_problems(t) -> void:
	var map := _level(t)
	var level := map.owner
	map.position = Vector3(64, -8, 32)
	map.rotation_degrees = Vector3(0, 30, 0)
	var report := map.build_from_text(_map_text([DATA]))
	var warnings := _terrain3d_warnings(report)
	t.check(warnings.size() == 1 and warnings[0].contains("origin") and warnings[0].contains("Map"), "a map away from the origin is reported, got %s" % [warnings])
	map.transform = Transform3D.IDENTITY

	for folder in ["", "  "]:
		var empty := _level(t, _map_text([folder]))
		warnings = _terrain3d_warnings(empty.build_report)
		var slot := TrenchDotTerrain3D.find(empty)
		t.check(slot != null and slot.get_terrain() == null and warnings.size() == 1 and warnings[0].contains("no data folder"), "a link without a folder %s gets a slot without Terrain3D and one warning, got %s" % [JSON.stringify(folder), warnings])
		empty.owner.free()

	var two := _level(t, _map_text([DATA, "res://tests/terrain3d/second", "res://tests/terrain3d/third"]))
	warnings = _terrain3d_warnings(two.build_report)
	var two_slot := TrenchDotTerrain3D.find(two)
	t.check(warnings.size() == 1 and warnings[0].contains("3 Terrain3D nodes") and warnings[0].contains(DATA), "several links are reported once, got %s" % [warnings])
	t.check(t.collect(two, func(n): return n is TrenchDotTerrain3D).size() == 1 and two_slot.get_terrain().get("data_directory") == DATA, "several links build one slot on the first folder")
	two.owner.free()
	level.free()

static func _test_without_terrain3d(t) -> void:
	TrenchDotTerrain3D.terrain_class = &"MissingTerrain3DClass"
	var map := _level(t, _map_text([DATA]))
	var warnings := _terrain3d_warnings(map.build_report)
	var slot := TrenchDotTerrain3D.find(map)
	t.check(map.get_node_or_null("entity_0_worldspawn") != null and slot != null and slot.get_child_count() == 0, "without Terrain3D the map builds with an empty slot")
	t.check(warnings.size() == 1 and warnings[0].contains("not installed") and warnings[0].contains(DATA), "without Terrain3D the build warns once, got %s" % [warnings])
	t.check(Array(slot._get_configuration_warnings()).any(func(w): return w.contains("not installed")), "the slot tells the Inspector Terrain3D is missing")
	t.check(is_nan(slot.height_at(SPOT.x, SPOT.y)), "a slot without Terrain3D has no height")
	TrenchDotTerrain3D.terrain_class = &"Terrain3D"
	if not ClassDB.class_exists("Terrain3D"):
		map.owner.free()
		return
	var report := map.build_from_text(_map_text([DATA]))
	t.check(slot.get_terrain() != null and _terrain3d_warnings(report).is_empty() and slot._get_configuration_warnings().is_empty(), "once Terrain3D is there the next build fills the slot")
	map.owner.free()

static func _test_live_session_and_hot_reload(t) -> void:
	var text := _map_text([DATA])
	var map := _level(t, text)
	var slot := TrenchDotTerrain3D.find(map)
	var terrain := slot.get_terrain()
	await t.process_frame
	var maps: Array[FuncGodotMap] = [map]
	var session := TrenchDotLiveSession.new(MAP, maps)
	t.check(session.begin(text), "the live session takes a linked map")
	var lamp_json: Dictionary = session._nodes[15].duplicate(true)
	lamp_json["properties"]["light_energy"] = "4"
	session.apply([{ "op": "set", "id": 15, "parent": 1, "index": 8, "node": lamp_json }])
	session.apply([{ "op": "translate", "ids": [7], "offset": [0.0, 32.0, 0.0] }])
	session.process()
	t.check(is_instance_valid(slot) and slot.get_terrain() == terrain and map.get_child(0) == slot, "live entity and world rebuilds keep the slot")
	var link: Dictionary = session._nodes[LINK_ID].duplicate(true)
	link["data_directory"] = "res://tests/terrain3d/relinked"
	session.apply([{ "op": "set", "id": LINK_ID, "parent": 1, "index": 11, "node": link }])
	session._last_change = -1000000
	session.process()
	var warnings := _terrain3d_warnings(map.build_report)
	t.check(is_instance_valid(terrain) and slot.get_terrain() == terrain and t.collect(map, func(n): return n is TrenchDotTerrain3D).size() == 1, "a live change of the link rebuilds the map and keeps the slot once")
	t.check(warnings.size() == 1 and warnings[0].contains("res://tests/terrain3d/relinked"), "a live relink reports the folder mismatch, got %s" % [warnings])

	var reloader := TrenchDotHotReload.new()
	reloader.port = 7898
	t.root.add_child(reloader)
	var rebuilt := reloader.reload(ProjectSettings.globalize_path(MAP))
	t.check(rebuilt >= 1 and is_instance_valid(slot) and slot.get_terrain() == terrain and map.get_child(0) == slot, "hot reload rebuilds the map and keeps the slot")
	reloader.free()
	map.owner.free()

static func _test_streamer_leaves_slot_alone(t) -> void:
	var map := FuncGodotMap.new()
	var slot := TrenchDotTerrain3D.new()
	map.add_child(slot)
	var big := MeshInstance3D.new()
	var plane := PlaneMesh.new()
	plane.size = Vector2(200, 200)
	var mesh := ArrayMesh.new()
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, plane.get_mesh_arrays())
	big.mesh = mesh
	slot.add_child(big)
	TrenchDotStreamer.split_visuals(map, 16.0)
	t.check(big.get_parent() == slot and big.mesh == mesh, "chunk streaming does not split what the slot holds")
	map.free()

static func _test_height_at(t) -> void:
	var loose := TrenchDotTerrain3D.new()
	t.root.add_child(loose)
	t.check(Array(loose._get_configuration_warnings()).any(func(w): return w.contains("FuncGodotMap")), "a slot outside a map tells the Inspector")
	loose.free()
	var map := _level(t, _map_text([DATA]))
	var slot := TrenchDotTerrain3D.find(map)
	await t.process_frame
	t.check(t.near(slot.height_at(VERTEX.x, VERTEX.y), 12.5), "height_at reads the height Terrain3D stored, got %s" % slot.height_at(VERTEX.x, VERTEX.y))
	t.check(t.near(slot.height_at(-200.0, 520.0), 0.0), "height_at reads blank ground in the region, got %s" % slot.height_at(-200.0, 520.0))
	t.check(t.near(slot.height_at(VERTEX.x + 0.5, VERTEX.y), 6.25), "height_at interpolates between two vertices, got %s" % slot.height_at(VERTEX.x + 0.5, VERTEX.y))
	t.check(t.near(slot.height_at(SPOT.x, SPOT.y), 12.5 * 0.5 * 0.75), "height_at interpolates between four vertices, got %s" % slot.height_at(SPOT.x, SPOT.y))
	var outside := slot.height_at(300.0, -40.0)
	t.check(is_nan(outside) or t.near(outside, 0.0), "height_at outside every region is no height or flat ground, got %s" % outside)
	map.owner.free()

## Terrain3D is optional, a script that names one of its classes fails to parse in a project without it.
static func _test_scripts_name_no_terrain3d_class(t) -> void:
	var word := RegEx.create_from_string("\\bTerrain3D\\w*")
	var strings := RegEx.create_from_string("&?\"(?:[^\"\\\\]|\\\\.)*\"")
	var offenders: Array[String] = []
	var stack: Array[String] = ["res://addons/func_godot"]
	var scanned := 0
	while not stack.is_empty():
		var dir: String = stack.pop_back()
		for sub in DirAccess.get_directories_at(dir):
			stack.append(dir.path_join(sub))
		for file in DirAccess.get_files_at(dir):
			if not file.ends_with(".gd"):
				continue
			scanned += 1
			var lines := FileAccess.get_file_as_string(dir.path_join(file)).split("\n")
			for i in lines.size():
				var code := strings.sub(lines[i], "\"\"", true)
				var hash := code.find("#")
				if hash >= 0:
					code = code.left(hash)
				if word.search(code):
					offenders.append("%s:%d" % [dir.path_join(file), i + 1])
	t.check(scanned > 100 and offenders.is_empty(), "no addon script names a Terrain3D class, found %s" % [offenders])
