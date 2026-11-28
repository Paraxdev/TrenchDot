extends RefCounted
## Scatter that follows Terrain3D: a set whose only target is the map's terrain3d node gets every instance put on
## Terrain3D's surface when the map builds. Instances over a hole or outside every region keep their stored height and
## the build report counts them.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const Export := preload("res://tests/terrain3d_export_tests.gd")
const Link := preload("res://tests/terrain3d_tests.gd")
const WORK := "user://td_t3d_scatter"
const SIZE := 64
const SPACING := 2.5
const PINE := "res://demo/models/pine.bbmodel"

static func run(t) -> void:
	print("- scatter that follows Terrain3D")
	if not ClassDB.class_exists("Terrain3D"):
		return
	await _test_follows(t)
	await _test_fresh_terrain(t)
	_test_first_frame(t)
	Export._clear(WORK)

## Heights that rise to the east and fall to the south, a hole of 3 x 3 vertices around HOLE in region (-1, 0).
static func _world(t) -> Node3D:
	var folder := WORK.path_join("follows")
	var terrain: Node3D = await Export._terrain(t, folder, SIZE, SPACING)
	for loc in [Vector2i(-1, 0), Vector2i(0, -1)]:
		var heights := PackedByteArray()
		var control := PackedByteArray()
		heights.resize(SIZE * SIZE * 4)
		control.resize(SIZE * SIZE * 4)
		for z in SIZE:
			for x in SIZE:
				heights.encode_float((z * SIZE + x) * 4, 7.0 + loc.y * 5.0 + x * 0.6 - z * 0.35)
				var hole: bool = loc == Vector2i(-1, 0) and absi(x - 20) <= 1 and absi(z - 40) <= 1
				control.encode_u32((z * SIZE + x) * 4, 1 << 2 if hole else 0)
		Export._fill(terrain, loc, {
			"height": Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, heights),
			"control": Image.create_from_data(SIZE, SIZE, false, Image.FORMAT_RF, control),
		})
	terrain.get(&"data").update_maps(3, true, false)
	terrain.get(&"data").save_directory(folder)
	return terrain

static func _scatter(id: int, targets: Array, instances: Array) -> Dictionary:
	return { "id": id, "type": "scatter", "name": "set%d" % id, "kind": "props", "collision": "none", "chunk_size": 0, "items": [{ "source": PINE }], "instances": instances, "targets": targets }

## Origins of every scattered pine of [param scatter], in instance order, from the transforms kept for the restore
## since headless drops MultiMesh buffers.
static func _origins(scatter: Node) -> Array:
	var out: Array = []
	for child in scatter.get_children():
		var mmi := child as MultiMeshInstance3D
		if mmi:
			var buffer: PackedFloat32Array = mmi.get_meta(TrenchDotScatter.BUFFER_META, mmi.multimesh.buffer)
			for o in range(0, buffer.size(), 12):
				out.append(Vector3(buffer[o + 3], buffer[o + 7], buffer[o + 11]))
	return out

static func _scatter_node(map: Node, id: int) -> TrenchDotScatter:
	for node in map.find_children("*", "TrenchDotScatter", true, false):
		if node.get_meta(TrenchDotBuild.ID_META, -1) == id:
			return node
	return null

static func _test_follows(t) -> void:
	var settings: TrenchDotMapSettings = load(Link.SETTINGS)
	var scale := settings.scale_factor
	var terrain := await _world(t)
	var data: Object = terrain.get(&"data")
	var hole := Vector3(-160.0 + 20 * SPACING, 0.0, 40 * SPACING)
	t.check(is_nan(data.get_surface_height(hole)) and is_nan(data.get_surface_height(Vector3(300.0, 0.0, 300.0))), "the fixture has a hole and ground outside every region")

	# Positions in map units, with rotations and scales that differ per instance, across both regions.
	var on_ground := [
		[0, -4400, 96, 1310, 0, 35, 0, 1.0],
		[0, -1650, -40, 3700, 10, -70, 4, 1.5],
		[0, 1100, 500, -2300, 0, 0, 0, 0.75],
		[0, 4730, 3, -510, -6, 180, 0, 1.25],
	]
	var off_ground := [
		[0, int(hole.x / scale) + 3, 77, int(hole.z / scale) - 5, 0, 20, 0, 1.0],
		[0, 9600, -12, 9600, 0, 0, 0, 1.0],
	]
	var json: Dictionary = JSON.parse_string(Link._map_text([WORK.path_join("follows")]))
	var link := Link.LINK_ID
	var children: Array = json["layers"][0]["children"]
	children.append(_scatter(101, [link], on_ground + off_ground))
	children.append(_scatter(102, [link, 1], on_ground))
	children.append(_scatter(103, [link], on_ground.slice(1, 2)))
	children.append(_scatter(104, [link], []))
	children.append(_scatter(105, [], on_ground))
	children.append(_scatter(106, [link + 7], on_ground))
	var far := []
	for k in 5:
		far.append([0, 9600 + k * 100, 0, 9600, 0, 0, 0, 1.0])
	children.append(_scatter(107, [link], far))

	var level := Node3D.new()
	t.root.add_child(level)
	var map := TrenchDotMap.new()
	map.map_settings = settings
	map.local_map_file = Link.MAP
	level.add_child(map)
	var slot := TrenchDotTerrain3D.new()
	slot.name = TrenchDotTerrain3D.NODE_NAME
	map.add_child(slot)
	terrain.get_parent().remove_child(terrain)
	slot.add_child(terrain)
	await t.process_frame
	var report: Dictionary = map.build_from_text(JSON.stringify(json))

	var reference := TrenchDotScatter.create(_scatter(101, [], on_ground + off_ground), Transform3D.IDENTITY, settings)
	var stored := _origins(reference)
	reference.free()
	var want: Array = []
	for i in stored.size():
		var raw: Array = (on_ground + off_ground)[i]
		var h: float = data.get_surface_height(Vector3(raw[1], 0.0, raw[3]) * scale)
		want.append(stored[i] if is_nan(h) else stored[i] + Vector3.UP * (h - raw[2] * scale))
	var got := _origins(_scatter_node(map, 101))
	var matches := got.size() == want.size()
	for i in mini(got.size(), want.size()):
		matches = matches and t.near(got[i].distance_to(want[i]), 0.0)
	t.check(matches, "a set that targets only the Terrain3D node sits on its surface, got %s want %s" % [got, want])
	t.check(got.size() == 6 and t.near(got[4].y, stored[4].y) and t.near(got[5].y, stored[5].y), "instances over a hole or no region keep their stored height")
	var ys: Array = on_ground.map(func(raw: Array) -> float: return data.get_surface_height(Vector3(raw[1], 0.0, raw[3]) * scale))
	t.check(ys.all(func(y: float) -> bool: return not is_nan(y)) and absf(ys[0] - ys[3]) > 1.0, "the ground under the set is slanted, got %s" % [ys])

	var counted := Array(report.get("warnings", [])).filter(func(w: String) -> bool: return w.contains("Terrain3D") and w.contains("set101"))
	t.check(counted.size() == 1 and counted[0].contains("2 instances") and counted[0].contains("set101"), "the report counts the two instances that keep their height, got %s" % [counted])
	var places := PackedStringArray()
	for k in [4, 5]:
		places.append("x %.0f, z %.0f map units (%.1f, %.1f m from the map node)" % [stored[k].x / scale, stored[k].z / scale, stored[k].x, stored[k].z])
	t.check(counted.size() == 1 and counted[0].contains("the first at " + "; ".join(places)), "the report says where the two instances are, got %s want %s" % [counted, places])

	var capped := Array(report.get("warnings", [])).filter(func(w: String) -> bool: return w.contains("set107"))
	var named := range(5).map(func(k: int) -> bool: return capped.size() == 1 and capped[0].contains("x %d, z 9600 map units (%.1f, %.1f m from the map node)" % [9600 + k * 100, (9600 + k * 100) * scale, 9600 * scale]))
	t.check(capped.size() == 1 and capped[0].contains("5 instances") and named == [true, true, true, false, false], "a set with five off-ground instances names the first three, got %s" % [capped])

	var stored_once := _origins(_scatter_node(map, 102))
	var untouched := TrenchDotScatter.create(_scatter(102, [], on_ground), Transform3D.IDENTITY, settings)
	t.check(stored_once == _origins(untouched), "a set with a second target keeps its stored heights")
	untouched.free()
	for id in [105, 106]:
		t.check(_origins(_scatter_node(map, id)) == stored_once, "set %d, without the Terrain3D node as its target, keeps its stored heights" % id)
	var one := _origins(_scatter_node(map, 103))
	t.check(one.size() == 1 and t.near(one[0].distance_to(want[1]), 0.0), "a set of one instance sits on the surface, got %s" % [one])
	t.check(_scatter_node(map, 104) != null and _origins(_scatter_node(map, 104)).is_empty(), "an empty set builds")

	# A live edit of the set builds it again on the surface.
	var live := TrenchDotScatter.build_one(map, map, _scatter(101, [link], on_ground), Transform3D.IDENTITY, 101, settings)
	var live_got := _origins(live)
	t.check(live_got.size() == 4 and t.near(live_got[3].distance_to(want[3]), 0.0), "a live edit of the set lands on the surface, got %s" % [live_got])
	var in_prefab := TrenchDotScatter.build_one(map, map, _scatter(101, [link], on_ground), Transform3D.IDENTITY, 1000101, settings)
	t.check(_origins(in_prefab) == stored_once, "a set inside a prefab instance whose target has the link's id keeps its stored heights")
	slot.remove_child(terrain)
	var bare := TrenchDotScatter.build_one(map, map, _scatter(101, [link], on_ground), Transform3D.IDENTITY, 101, settings)
	t.check(_origins(bare) == stored_once and bare.ungrounded == 0, "without a Terrain3D in the slot a set keeps its stored heights and counts nothing")
	slot.add_child(terrain)
	map.build()
	t.check(not slot.has_meta(TrenchDotTerrain3D.LINK_META) and _origins(TrenchDotScatter.build_one(map, map, _scatter(101, [link], on_ground), Transform3D.IDENTITY, 101, settings)) == stored_once, "after a build without the link the slot follows no id")
	level.free()

## A map built for the first time creates its Terrain3D, which must be ready before the scatter step.
static func _test_fresh_terrain(t) -> void:
	var settings: TrenchDotMapSettings = load(Link.SETTINGS)
	var json: Dictionary = JSON.parse_string(Link._map_text([Link.DATA]))
	var raw := [0, int(Link.VERTEX.x * settings.inverse_scale_factor), 40, int(Link.VERTEX.y * settings.inverse_scale_factor), 0, 0, 0, 1.0]
	json["layers"][0]["children"].append(_scatter(101, [Link.LINK_ID], [raw]))
	var map := Link._level(t, JSON.stringify(json))
	var reference := TrenchDotScatter.create(_scatter(101, [], [raw]), Transform3D.IDENTITY, settings)
	var got := _origins(_scatter_node(map, 101))
	var stored := _origins(reference)
	reference.free()
	t.check(got.size() == 1 and t.near(got[0].y, stored[0].y - 40 * settings.scale_factor + 12.5), "the first build of a map puts its scatter on the Terrain3D it creates, got %s" % [got])
	map.owner.free()

## A map built before its Terrain3D has been ready in the tree leaves the scatter at its stored height and says why.
## Runs in its own Godot, since this crashed Godot before.
static func _test_first_frame(t) -> void:
	var output: Array = []
	var args := ["--headless", "--path", ProjectSettings.globalize_path("res://"), "--script", "res://tests/helpers/terrain3d_first_frame.gd"]
	var code := OS.execute(OS.get_executable_path(), args, output, true)
	var lines := "\n".join(output).strip_edges().split("\n")
	var got: Variant = JSON.parse_string(lines[lines.size() - 1])
	var warnings: Array = got.get("warnings", []) if got is Dictionary else []
	t.check(code == 0 and got is Dictionary and got.get("origins", []).size() == 1, "a map built in the first frame keeps its scatter, got %d %s" % [code, lines[lines.size() - 1]])
	t.check(warnings.size() == 1 and str(warnings[0]).contains("not ready"), "and its build report says the Terrain3D was not ready, got %s" % [warnings])
