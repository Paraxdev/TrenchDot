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
	await _test_adopts_folder_resources(t)
	await _test_build_adopts_folder_resources(t)
	await _test_build_takes_folder_ocean(t)
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

static func _level(t, text := "") -> TrenchDotMap:
	var level := Node3D.new()
	level.name = "Level"
	t.root.add_child(level)
	var map := TrenchDotMap.new()
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

static func _parse(json: Variant) -> TrenchDotData.ParseData:
	return TrenchDotParser.parse_dict(json, load(SETTINGS), TrenchDotData.ParseData.new(), "terrain3d.tdm")

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
		ctx.parse_data = TrenchDotData.ParseData.new()
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
## regions keeps the size and spacing of its files.
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

	var spaced := "user://td_t3d_new_size_spaced"
	Apply.new_folder(spaced, 128, 2.5)
	DirAccess.copy_absolute("res://tests/terrain3d/v1_0_2/terrain3d_02-01.res", spaced + "/terrain3d_02-01.res")
	var built := _level(t, _map_text([spaced]))
	await t.process_frame
	var node := (built.get_child(0) as TrenchDotTerrain3D).get_terrain()
	t.check(node != null and t.near(float(node.get("vertex_spacing")), 2.0), "a folder with regions builds a Terrain3D of the spacing its files were saved with, not the manifest's, got %s" % [node.get("vertex_spacing") if node else null])
	built.owner.free()
	Apply.Export._clear(spaced)

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

## A Terrain3D in the tree on [param folder], with the blank material and assets Terrain3D gives it.
static func _adopter(t, folder: String) -> Node3D:
	var terrain := _new_terrain(folder)
	t.root.add_child(terrain)
	await t.process_frame
	return terrain

## Saves each resource in [param files] under its file name in a fresh [param folder], next to a copy of the probe
## region and a text file that only looks like a resource.
static func _resource_folder(folder: String, files: Dictionary) -> void:
	Apply.Export._clear(folder)
	DirAccess.make_dir_recursive_absolute(folder)
	DirAccess.copy_absolute(DATA + "/terrain3d-01_02.res", folder + "/terrain3d-01_02.res")
	FileAccess.open(folder + "/notes.txt", FileAccess.WRITE).store_string("[gd_resource type=\"Terrain3DAssets\"]")
	for file: String in files:
		ResourceSaver.save(files[file], folder.path_join(file))

static func _assets(names: Array, meshes := 0) -> Resource:
	var assets: Resource = ClassDB.instantiate(&"Terrain3DAssets")
	for i in names.size():
		var texture: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
		texture.set(&"name", names[i])
		assets.call(&"set_texture_asset", i, texture)
	for i in meshes:
		assets.call(&"set_mesh_asset", i, ClassDB.instantiate(&"Terrain3DMeshAsset"))
	return assets

static func _material(background: int) -> Resource:
	var material: Resource = ClassDB.instantiate(&"Terrain3DMaterial")
	material.set(&"world_background", background)
	return material

static func _texture_names(terrain: Node3D) -> Array:
	var assets: Object = terrain.get(&"assets") if terrain else null
	return Array(assets.call(&"get_texture_list")).map(func(a): return str(a.get(&"name"))) if assets else []

static func _path_of(terrain: Node3D, property: StringName) -> String:
	var resource: Resource = terrain.get(property) if terrain else null
	return resource.resource_path if resource else "<none>"

static func _test_adopts_folder_resources(t) -> void:
	var folder := "user://td_t3d_adopt"

	_resource_folder(folder + "_none", {})
	var bare := await _adopter(t, folder + "_none")
	var blank_material: Resource = bare.get(&"material")
	var blank_assets: Resource = bare.get(&"assets")
	var problems := TrenchDotTerrain3DCache.adopt_folder_resources(bare, folder + "_none", true)
	t.check(problems.is_empty() and bare.get(&"material") == blank_material and bare.get(&"assets") == blank_assets, "a folder without resources leaves a fresh node as it was, got %s" % [problems])
	bare.free()

	_resource_folder(folder + "_one", { "M_terrain.tres": _material(2), "assets.tres": _assets(["Cliff", "Grass"]) })
	var fresh := await _adopter(t, folder + "_one")
	var ocean_material: Variant = fresh.get(&"ocean_material")
	problems = TrenchDotTerrain3DCache.adopt_folder_resources(fresh, folder + "_one", true)
	t.check(problems.is_empty() and _path_of(fresh, &"material") == folder + "_one/M_terrain.tres" and int(fresh.get(&"material").get(&"world_background")) == 2, "a fresh node gets the folder's material, got %s %s" % [problems, _path_of(fresh, &"material")])
	t.check(_path_of(fresh, &"assets") == folder + "_one/assets.tres" and _texture_names(fresh) == ["Cliff", "Grass"], "a fresh node gets the folder's assets, got %s %s" % [_path_of(fresh, &"assets"), _texture_names(fresh)])
	t.check(fresh.get(&"ocean_enabled") == false and fresh.get(&"ocean_material") == ocean_material, "adopting leaves the ocean alone")
	fresh.free()

	_resource_folder(folder + "_two", { "b_assets.tres": _assets(["Second"]), "a_assets.tres": _assets(["First"]), "M_terrain.tres": _material(1) })
	var two := await _adopter(t, folder + "_two")
	problems = TrenchDotTerrain3DCache.adopt_folder_resources(two, folder + "_two", true)
	t.check(_texture_names(two) == ["First"] and _path_of(two, &"assets").ends_with("/a_assets.tres") and _path_of(two, &"material").ends_with("/M_terrain.tres"), "with two assets files the first by name wins, got %s" % [_path_of(two, &"assets")])
	t.check(problems.size() == 1 and problems[0].contains("a_assets.tres") and problems[0].contains("b_assets.tres") and problems[0].contains("Terrain3DAssets"), "two assets files give one problem naming both, got %s" % [problems])
	two.free()

	_resource_folder(folder + "_res", { "look.res": _material(2), "set.res": _assets(["Binary"]) })
	var binary := await _adopter(t, folder + "_res")
	problems = TrenchDotTerrain3DCache.adopt_folder_resources(binary, folder + "_res", true)
	t.check(problems.is_empty() and _path_of(binary, &"material").ends_with("/look.res") and _texture_names(binary) == ["Binary"], ".res files are found by their class and region files are skipped, got %s %s %s" % [problems, _path_of(binary, &"material"), _texture_names(binary)])
	binary.free()

	var mine := await _adopter(t, folder + "_one")
	mine.set(&"assets", _assets(["Mine"]))
	var embedded: Resource = mine.get(&"material")
	problems = TrenchDotTerrain3DCache.adopt_folder_resources(mine, folder + "_one", false)
	t.check(problems.is_empty() and _texture_names(mine) == ["Mine"] and mine.get(&"material") == embedded, "an existing node keeps the user's assets and with them its material, got %s" % [_texture_names(mine)])
	mine.set(&"assets", _assets([], 2))
	TrenchDotTerrain3DCache.adopt_folder_resources(mine, folder + "_one", false)
	t.check(_texture_names(mine).is_empty() and int(mine.get(&"assets").call(&"get_mesh_count")) == 2 and mine.get(&"material") == embedded, "assets with two texture cards and no texture are kept")
	var rock := Node3D.new()
	var shape := MeshInstance3D.new()
	shape.mesh = BoxMesh.new()
	rock.add_child(shape)
	shape.owner = rock
	var scene := PackedScene.new()
	scene.pack(rock)
	rock.free()
	var single := _assets([])
	var mesh: Resource = ClassDB.instantiate(&"Terrain3DMeshAsset")
	mesh.set(&"scene_file", scene)
	single.call(&"set_mesh_asset", 0, mesh)
	mine.set(&"assets", single)
	TrenchDotTerrain3DCache.adopt_folder_resources(mine, folder + "_one", false)
	t.check(mine.get(&"assets") == single and mine.get(&"material") == embedded, "assets with one mesh made from a scene are kept")
	mine.free()

	var empty := await _adopter(t, folder + "_one")
	empty.set(&"assets", _assets([]))
	TrenchDotTerrain3DCache.adopt_folder_resources(empty, folder + "_one", false)
	t.check(_texture_names(empty) == ["Cliff", "Grass"] and _path_of(empty, &"material") == folder + "_one/M_terrain.tres", "an existing node with empty assets adopts the folder's assets and material, got %s %s" % [_texture_names(empty), _path_of(empty, &"material")])
	empty.free()

	var own := await _adopter(t, folder + "_one")
	ResourceSaver.save(_material(1), folder + "_none/own_material.tres")
	own.set(&"assets", _assets([]))
	own.set(&"material", load(folder + "_none/own_material.tres"))
	TrenchDotTerrain3DCache.adopt_folder_resources(own, folder + "_one", false)
	t.check(_texture_names(own) == ["Cliff", "Grass"] and _path_of(own, &"material") == folder + "_none/own_material.tres", "a material saved as its own file is kept while the assets are adopted, got %s" % [_path_of(own, &"material")])
	own.free()

	var detached: Node3D = _new_terrain(folder + "_res")
	TrenchDotTerrain3DCache.adopt_folder_resources(detached, folder + "_res", false)
	t.check(_texture_names(detached) == ["Binary"] and _path_of(detached, &"material").ends_with("/look.res"), "a node without assets or material adopts both even when not fresh, got %s" % [_path_of(detached, &"material")])
	detached.free()

	for suffix in ["_none", "_one", "_two", "_res"]:
		Apply.Export._clear(folder + suffix)

## A build gives the Terrain3D it creates the folder's resources and reports a second assets file, a rebuild keeps the
## assets the user set on it since and fills empty ones.
static func _test_build_adopts_folder_resources(t) -> void:
	var folder := "user://td_t3d_build_adopt"
	_resource_folder(folder, { "M_terrain.tres": _material(2), "assets.tres": _assets(["Cliff", "Grass"]), "z_assets.tres": _assets(["Late"]) })
	var map := _level(t, _map_text([folder]))
	await t.process_frame
	var terrain := (map.get_child(0) as TrenchDotTerrain3D).get_terrain()
	var warnings := _terrain3d_warnings(map.build_report)
	t.check(terrain != null and _path_of(terrain, &"material") == folder + "/M_terrain.tres" and _texture_names(terrain) == ["Cliff", "Grass"] and terrain.get(&"ocean_enabled") == false, "the built Terrain3D has the folder's material and assets and no ocean, got %s" % [_texture_names(terrain)])
	t.check(warnings.size() == 1 and warnings[0].contains("z_assets.tres"), "the build reports the second assets file, got %s" % [warnings])
	if not terrain:
		map.owner.free()
		return
	terrain.set(&"assets", _assets(["Mine"]))
	map.build_from_text(_map_text([folder]))
	await t.process_frame
	t.check(_texture_names(terrain) == ["Mine"] and _path_of(terrain, &"material") == folder + "/M_terrain.tres", "a rebuild keeps the assets the user set, got %s" % [_texture_names(terrain)])
	terrain.set(&"assets", _assets([]))
	map.build_from_text(_map_text([folder]))
	t.check(_texture_names(terrain) == ["Cliff", "Grass"], "a rebuild fills empty assets, got %s" % [_texture_names(terrain)])
	map.owner.free()
	Apply.Export._clear(folder)

## A node a build creates has the ocean on when the folder's manifest says its Terrain3D had one. A node that exists
## keeps the choice made on it, and a folder whose manifest has no ocean or an ocean that is off builds without one.
static func _test_build_takes_folder_ocean(t) -> void:
	var cases := { "_on": true, "_off": false, "_none": null }
	for suffix: String in cases:
		var folder := "user://td_t3d_build_ocean" + suffix
		_resource_folder(folder, { "M_terrain.tres": _material(2) })
		var manifest := { "format": TrenchDotTerrain3DCache.FORMAT, "version": TrenchDotTerrain3DCache.VERSION }
		if cases[suffix] != null:
			manifest["ocean"] = { "enabled": cases[suffix], "sea_level": 3.5, "water_color": [0.1, 0.2, 0.3, 1.0], "source": "" }
		TrenchDotTerrain3DCache.write_file(TrenchDotTerrain3DCache.cache_dir(folder).path_join(TrenchDotTerrain3DCache.MANIFEST), JSON.stringify(manifest).to_utf8_buffer())
		var map := _level(t, _map_text([folder]))
		await t.process_frame
		var terrain := (map.get_child(0) as TrenchDotTerrain3D).get_terrain()
		t.check(terrain != null and terrain.get(&"ocean_enabled") == (cases[suffix] == true), "a new node has the ocean %s when the folder's manifest says %s, got %s" % [cases[suffix] == true, cases[suffix], terrain.get(&"ocean_enabled") if terrain else null])
		if terrain and suffix == "_on":
			terrain.set(&"ocean_enabled", false)
			map.build_from_text(_map_text([folder]))
			t.check(terrain.get(&"ocean_enabled") == false, "a rebuild keeps an ocean the user switched off on the node")
			terrain.set(&"ocean_enabled", true)
			map.build_from_text(_map_text([folder]))
			t.check(terrain.get(&"ocean_enabled") == true, "a rebuild keeps an ocean the user switched on on the node")
		map.owner.free()
		Apply.Export._clear(folder)

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
	var maps: Array[TrenchDotMap] = [map]
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
	var map := TrenchDotMap.new()
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
	t.check(Array(loose._get_configuration_warnings()).any(func(w): return w.contains("TrenchDotMap")), "a slot outside a map tells the Inspector")
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
	var stack: Array[String] = ["res://addons/trenchdot"]
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
