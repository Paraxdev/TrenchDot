extends RefCounted
## Tests for [TrenchDotVoidCull], the face cull of sealed maps. res://tests/run_tests.gd calls [method run], [param t]
## is that script's own instance, used for its [code]check[/code] helper.

const SETTINGS := "res://demo/demo_map_settings.tres"
const Parity := preload("res://tests/td_editor_parity_test.gd")

class WarningLog extends Logger:
	var warnings: Array[String] = []

	func _log_error(function: String, file: String, line: int, code: String, rationale: String, editor_notify: bool, error_type: int, script_backtraces: Array[ScriptBacktrace]) -> void:
		if error_type == Logger.ERROR_TYPE_WARNING:
			warnings.append(code + " " + rationale)

static func run(t) -> void:
	_test_sealed_room_drops_outer_faces(t)
	_test_seed_outside_the_walls_leaks(t)
	_test_leaking_room_keeps_everything(t)
	_test_switches(t)
	_test_seam_does_not_leak(t)
	_test_mesh_walls_seal(t)
	_test_sky_walls_seal(t)
	_test_moving_entities_do_not_seal(t)
	_test_gap_keeps_faces_beyond(t)
	_test_switch_values(t)
	_test_size_of_the_gain(t)
	_test_default_is_off(t)
	_test_empty_solid_entity_is_no_seed(t)
	_test_see_through_faces_are_not_candidates(t)

static func _node(id: int, mn: Vector3, mx: Vector3, type := "brush", material := "showcase/cobble") -> Dictionary:
	var node := Parity._box(id, mn, mx, material)
	node["type"] = type
	return node

static func _start(id: int, at: Vector3) -> Dictionary:
	return { "type": "entity", "id": id, "classname": "info_player_start", "origin": [at.x, at.y, at.z], "angles": [0.0, 0.0, 0.0], "properties": {} }

static func _entity(id: int, classname: String, properties: Dictionary, children: Array) -> Dictionary:
	return { "type": "entity", "id": id, "classname": classname, "origin": [0.0, 0.0, 0.0], "angles": [0.0, 0.0, 0.0], "properties": properties, "children": children }

## Slabs 16 thick around [param inner], leaving out those named in [param skip] ("-x", "+x", "-y", ...).
static func _shell(inner: AABB, first_id: int, skip := [], type := "brush", material := "showcase/cobble") -> Array:
	var a := inner.position
	var b := inner.end
	var big_a := a - Vector3.ONE * 16.0
	var big_b := b + Vector3.ONE * 16.0
	var slabs := {
		"-y": [big_a, Vector3(big_b.x, a.y, big_b.z)],
		"+y": [Vector3(big_a.x, b.y, big_a.z), big_b],
		"-x": [Vector3(big_a.x, a.y, big_a.z), Vector3(a.x, b.y, big_b.z)],
		"+x": [Vector3(b.x, a.y, big_a.z), Vector3(big_b.x, b.y, big_b.z)],
		"-z": [Vector3(a.x, a.y, big_a.z), Vector3(b.x, b.y, a.z)],
		"+z": [Vector3(a.x, a.y, b.z), Vector3(b.x, b.y, big_b.z)],
	}
	var out := []
	var id := first_id
	for key in slabs:
		if not key in skip:
			out.append(_node(id, slabs[key][0], slabs[key][1], type, material))
		id += 1
	return out

## The culling is off by default, so a map turns it on unless [param keyed] is false.
static func _map(children: Array, properties := {}, keyed := true) -> Dictionary:
	if keyed and not properties.has("_cull_void_faces"):
		properties = properties.merged({ "_cull_void_faces": "1" })
	return { "format": "trenchdot-map", "properties": properties, "layers": [{ "type": "layer", "id": 1, "children": children }] }

## Builds [param map_json] like a map build does, up to the face cull.
static func _build(map_json: Dictionary, settings: FuncGodotMapSettings = null) -> Array:
	if not settings:
		settings = load(SETTINGS)
	var path := OS.get_temp_dir().path_join("td_void_cull_test.tdm")
	var data := FuncGodotParser.new().parse_map_data(Parity._write("td_void_cull_test.tdm", map_json), settings)
	DirAccess.remove_absolute(path)
	var generator := FuncGodotGeometryGenerator.new(settings)
	generator.map_path = "void_test"
	generator.build(0, data.entities)
	return [data, generator]

static func _shown(brush: FuncGodotData.BrushData) -> int:
	return brush.faces.filter(func(f): return not f.render_hidden).size()

static func _shown_all(entity: FuncGodotData.EntityData) -> int:
	var n := 0
	for brush in entity.brushes:
		n += _shown(brush)
	return n

## Whether the face of [param brush] looking along [param dir] (map space, y up) draws.
static func _draws(brush: FuncGodotData.BrushData, dir: Vector3) -> bool:
	var id_dir := Vector3(dir.z, dir.x, dir.y)
	for f in brush.faces:
		if f.plane.normal.is_equal_approx(id_dir):
			return not f.render_hidden
	return false

static func _room(extra := [], skip := [], properties := {}, type := "brush", start := Vector3(128, 64, 128)) -> Array:
	var children := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10, skip, type)
	children.append(_start(50, start))
	children.append_array(extra)
	return _build(_map(children, properties))

static func _test_sealed_room_drops_outer_faces(t) -> void:
	print("- a sealed room drops the faces of its walls that face the void")
	var data: FuncGodotData.ParseData = _room()[0]
	var brushes := data.entities[0].brushes
	# Slabs in the order of _shell: floor, ceiling, west, east, north, south. Their inner faces look into the room.
	var inner := [Vector3.UP, Vector3.DOWN, Vector3.RIGHT, Vector3.LEFT, Vector3.BACK, Vector3.FORWARD]
	var outer_hidden := true
	var inner_shown := true
	for i in 6:
		inner_shown = inner_shown and _draws(brushes[i], inner[i])
		outer_hidden = outer_hidden and not _draws(brushes[i], -inner[i])
	t.check(inner_shown, "the six faces looking into the room draw")
	t.check(outer_hidden, "the six faces looking out of the room are hidden")
	t.check(_shown_all(data.entities[0]) == 6, "nothing else of the room draws, got %d faces" % _shown_all(data.entities[0]))

static func _test_seed_outside_the_walls_leaks(t) -> void:
	print("- an entity beyond every wall leaks and keeps every face")
	var off: FuncGodotData.ParseData = _room([], [], { "_cull_void_faces": "0" })[0]
	# The start inside floods first, the one beyond the walls must still turn the map into a leak.
	var data: FuncGodotData.ParseData = _room([_start(51, Vector3(128, 64, 900))])[0]
	t.check(_shown_all(data.entities[0]) == _shown_all(off.entities[0]), "every face draws, %d of %d" % [_shown_all(data.entities[0]), _shown_all(off.entities[0])])

static func _test_leaking_room_keeps_everything(t) -> void:
	print("- a room with a hole leaks, keeps every face and warns")
	var log := WarningLog.new()
	OS.add_logger(log)
	var sealed: Array = _room()
	var kept_sealed := _shown_all(sealed[0].entities[0])
	var clean := log.warnings.filter(func(w): return w.contains("leaks")).is_empty()
	var leaky: Array = _room([], ["-z"])
	OS.remove_logger(log)
	t.check(clean, "a sealed room does not warn")
	var data: FuncGodotData.ParseData = leaky[0]
	var covered := data.entities[0].brushes.size() * 6
	t.check(_shown_all(data.entities[0]) >= covered - 12, "a leaking map keeps nearly every face (%d of %d)" % [_shown_all(data.entities[0]), covered])
	var warned := log.warnings.filter(func(w): return w.contains("leaks") and w.contains("info_player_start") and w.contains("void faces are kept"))
	t.check(warned.size() == 1, "a leak names the entity once, got %s" % [log.warnings])
	t.check(kept_sealed == 6, "the same room sealed draws only its six inner faces")
	# The leak keeps the outer faces of the walls the hole does not touch.
	var floor_brush: FuncGodotData.BrushData = data.entities[0].brushes[0]
	t.check(_draws(floor_brush, Vector3.DOWN), "the outer face of the floor draws in a leaking map")

static func _test_switches(t) -> void:
	print("- the worldspawn switch and the entity override keep faces")
	var off: Array = _room([], [], { "_cull_void_faces": "0" })
	var off_brushes = off[0].entities[0].brushes
	t.check(_draws(off_brushes[0], Vector3.DOWN) and _draws(off_brushes[2], Vector3.LEFT), "worldspawn off keeps the outer faces")

	# The east wall as a func_detail brush entity, with and without the override.
	for override in [null, "0", "1"]:
		var props := {} if override == null else { "_cull_void_faces": override }
		var shell := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10, ["+x"])
		var east := _node(30, Vector3(256, 0, -16), Vector3(272, 128, 272))
		shell.append(_entity(40, "func_detail", props, [east]))
		shell.append(_start(50, Vector3(128, 64, 128)))
		var data: FuncGodotData.ParseData = _build(_map(shell))[0]
		var wall: FuncGodotData.BrushData = data.entities[1].brushes[0]
		var outer_draws := _draws(wall, Vector3.RIGHT)
		t.check(_draws(wall, Vector3.LEFT), "the inner face of a detail wall draws")
		t.check(outer_draws == (override == "0"), "a detail wall with the switch %s %s its outer face" % [override, "keeps" if outer_draws else "drops"])
		var world_hidden := not _draws(data.entities[0].brushes[0], Vector3.DOWN)
		t.check(world_hidden, "worldspawn faces are still culled next to an entity that keeps its own")

	var shell := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10, ["+x"])
	shell.append(_entity(40, "func_detail", { "_cull_void_faces": "1" }, [_node(30, Vector3(256, 0, -16), Vector3(272, 128, 272))]))
	shell.append(_start(50, Vector3(128, 64, 128)))
	var data: FuncGodotData.ParseData = _build(_map(shell, { "_cull_void_faces": "0" }))[0]
	t.check(_draws(data.entities[1].brushes[0], Vector3.RIGHT) and _draws(data.entities[0].brushes[0], Vector3.DOWN), "an entity switch on does nothing while worldspawn is off")

static func _test_seam_does_not_leak(t) -> void:
	print("- walls of two brushes meeting at a seam do not leak")
	for seam in [128.0, 100.5, 96.0]:
		var north_a := _node(31, Vector3(0, 0, -16), Vector3(seam, 128, 0))
		var north_b := _node(32, Vector3(seam, 0, -16), Vector3(256, 128, 0))
		var data: FuncGodotData.ParseData = _room([north_a, north_b], ["-z"])[0]
		var brushes := data.entities[0].brushes
		var west_wall := brushes[2]
		t.check(not _draws(brushes[0], Vector3.DOWN) and not _draws(west_wall, Vector3.LEFT), "the room with a seam at %s stays sealed, outer faces hide" % seam)
		t.check(_draws(brushes[brushes.size() - 2], Vector3.FORWARD) or _draws(brushes[brushes.size() - 2], Vector3.BACK), "the seamed wall keeps its inner face")

static func _test_mesh_walls_seal(t) -> void:
	print("- closed mesh walls seal")
	var data: FuncGodotData.ParseData = _room([], [], {}, "mesh")[0]
	var brushes := data.entities[0].brushes
	t.check(brushes.all(func(b): return b.is_mesh and b.closed), "the slabs are closed meshes")
	t.check(_draws(brushes[0], Vector3.UP) and not _draws(brushes[0], Vector3.DOWN) and not _draws(brushes[3], Vector3.RIGHT), "a room of mesh walls drops its outer faces")
	var mixed := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10, ["-z"])
	mixed.append(_node(20, Vector3(0, 0, -16), Vector3(256, 128, 0), "mesh"))
	mixed.append(_start(50, Vector3(128, 64, 128)))
	var mixed_data: FuncGodotData.ParseData = _build(_map(mixed))[0]
	t.check(not _draws(mixed_data.entities[0].brushes[0], Vector3.DOWN), "one mesh wall among brush walls seals the room")

static func _test_sky_walls_seal(t) -> void:
	print("- sky brushes seal like walls")
	var settings: FuncGodotMapSettings = load(SETTINGS)
	var children := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10, ["+y"])
	children.append(_node(20, Vector3(-16, 128, -16), Vector3(272, 144, 272), "brush", settings.sky_texture))
	children.append(_start(50, Vector3(128, 64, 128)))
	var data: FuncGodotData.ParseData = _build(_map(children))[0]
	t.check(not _draws(data.entities[0].brushes[0], Vector3.DOWN) and not _draws(data.entities[0].brushes[1], Vector3.LEFT), "a room closed by a sky brush is sealed")

static func _test_moving_entities_do_not_seal(t) -> void:
	print("- moving entities do not seal")
	var shell := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10, ["-z"])
	shell.append(_entity(40, "func_rotating", {}, [_node(30, Vector3(0, 0, -16), Vector3(256, 128, 0))]))
	shell.append(_start(50, Vector3(128, 64, 128)))
	var log := WarningLog.new()
	OS.add_logger(log)
	var data: FuncGodotData.ParseData = _build(_map(shell))[0]
	OS.remove_logger(log)
	t.check(data.entities[1].definition is FuncGodotFGDSolidClass and data.entities[1].definition.node_class == "AnimatableBody3D", "func_rotating is a moving class")
	t.check(_draws(data.entities[0].brushes[0], Vector3.DOWN), "a door in the wall leaves the room open, the floor keeps its outer face")
	t.check(not log.warnings.filter(func(w): return w.contains("leaks")).is_empty(), "the open room warns")

static func _test_gap_keeps_faces_beyond(t) -> void:
	print("- a face seen through a gap wider than the lattice stays")
	for gap in [Vector2(96, 120), Vector2(0, 0)]:
		var children := []
		children.append(_node(31, Vector3(120, 0, 0), Vector3(136, 128, 256)) if gap.y == 0 else _node(31, Vector3(120, 0, 0), Vector3(136, 128, gap.x)))
		if gap.y > 0:
			children.append(_node(32, Vector3(120, 0, gap.y), Vector3(136, 128, 256)))
		var data: FuncGodotData.ParseData = _room(children, [], {}, "brush", Vector3(60, 64, 128))[0]
		var brushes := data.entities[0].brushes
		# Slab 3 is the east wall, its inner face looks west into the far half of the room.
		var far_face := _draws(brushes[3], Vector3.LEFT)
		if gap.y > 0:
			t.check(far_face, "the far wall stays visible through a 24 unit gap")
		else:
			t.check(not far_face, "a far half with no way in shows nothing")

static func _test_switch_values(t) -> void:
	print("- switch values")
	for pair in [[true, true], [false, false], [1, true], [0, false], ["1", true], ["0", false], ["false", false], ["True", true], ["", true], [null, true]]:
		t.check(TrenchDotVoidCull._switch(pair[0], true) == pair[1], "the switch reads %s as %s" % [pair[0], pair[1]])
	t.check(not TrenchDotVoidCull._switch(null, false), "an unset switch takes its fallback")

## Rooms in a grid, each joined to the next through doorways, with the player in the first.
static func _rooms(cols: int, rows: int) -> Dictionary:
	var width := cols * 272.0 - 16.0
	var depth := rows * 272.0 - 16.0
	var children := [
		_node(10, Vector3(-16, -16, -16), Vector3(width + 16, 0, depth + 16)),
		_node(11, Vector3(-16, 128, -16), Vector3(width + 16, 144, depth + 16)),
		_node(12, Vector3(-16, 0, -16), Vector3(0, 128, depth + 16)),
		_node(13, Vector3(width, 0, -16), Vector3(width + 16, 128, depth + 16)),
		_node(14, Vector3(0, 0, -16), Vector3(width, 128, 0)),
		_node(15, Vector3(0, 0, depth), Vector3(width, 128, depth + 16)),
	]
	var id := 20
	for cx in cols - 1:
		for cz in rows:
			var lo := Vector3(cx * 272 + 256, 0, cz * 272)
			children.append(_node(id, lo, lo + Vector3(16, 128, 80)))
			children.append(_node(id + 1, lo + Vector3(0, 0, 144), lo + Vector3(16, 128, 256)))
			children.append(_node(id + 2, lo + Vector3(0, 100, 80), lo + Vector3(16, 128, 144)))
			if cz < rows - 1:
				children.append(_node(id + 3, lo + Vector3(0, 0, 256), lo + Vector3(16, 128, 272)))
			id += 4
	for cz in rows - 1:
		for cx in cols:
			var lo := Vector3(cx * 272, 0, cz * 272 + 256)
			children.append(_node(id, lo, lo + Vector3(80, 128, 16)))
			children.append(_node(id + 1, lo + Vector3(144, 0, 0), lo + Vector3(256, 128, 16)))
			children.append(_node(id + 2, lo + Vector3(80, 100, 0), lo + Vector3(144, 128, 16)))
			if cx < cols - 1:
				children.append(_node(id + 3, lo + Vector3(256, 0, 0), lo + Vector3(272, 128, 16)))
			id += 4
	children.append(_start(9000, Vector3(128, 64, 128)))
	return _map(children)

static func _triangles(data: FuncGodotData.ParseData) -> int:
	var n := 0
	for entity in data.entities:
		for brush in entity.brushes:
			for f in brush.faces:
				if not f.render_hidden:
					n += maxi(f.vertices.size() - 2, 0)
	return n

static func _test_size_of_the_gain(t) -> void:
	print("- the gain on a sealed multi room map")
	var settings: FuncGodotMapSettings = load(SETTINGS)
	var off := _rooms(5, 4)
	off["properties"] = { "_cull_void_faces": "0" }
	var built := _build(off)
	var data: FuncGodotData.ParseData = built[0]
	var before := _triangles(data)
	var started := Time.get_ticks_usec()
	var hidden := TrenchDotVoidCull.apply(data.entities, settings, built[1].texture_materials, "rooms")
	var took := Time.get_ticks_usec() - started
	# The switch is read from worldspawn, so the pass above did nothing, flip it and run again.
	t.check(hidden == 0, "the pass is a no-op with worldspawn off")
	data.entities[0].properties["_cull_void_faces"] = "1"
	started = Time.get_ticks_usec()
	hidden = TrenchDotVoidCull.apply(data.entities, settings, built[1].texture_materials, "rooms")
	took = Time.get_ticks_usec() - started
	var after := _triangles(data)
	print("  void cull: %d triangles before, %d after, %d faces hidden, %.1f ms" % [before, after, hidden, took / 1000.0])
	t.check(hidden > 0 and after < before, "a sealed multi room map drops its void faces")

static func _test_default_is_off(t) -> void:
	print("- a map without the key culls nothing")
	var children := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10)
	children.append(_start(50, Vector3(128, 64, 128)))
	var data: FuncGodotData.ParseData = _build(_map(children, {}, false))[0]
	var brushes = data.entities[0].brushes
	t.check(_draws(brushes[0], Vector3.DOWN) and _draws(brushes[2], Vector3.LEFT), "the outer faces draw without the key")
	var settings: FuncGodotMapSettings = load(SETTINGS)
	t.check(TrenchDotVoidCull.apply(data.entities, settings, {}, "default") == 0, "the pass hides nothing without the key")

static func _test_empty_solid_entity_is_no_seed(t) -> void:
	print("- a brush entity that lost its brushes does not seed the flood")
	var children := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10)
	children.append(_start(50, Vector3(128, 64, 128)))
	var empty := _entity(40, "func_detail", {}, [])
	empty["origin"] = [2000.0, 0.0, 0.0]
	children.append(empty)
	var data: FuncGodotData.ParseData = _build(_map(children))[0]
	t.check(not _draws(data.entities[0].brushes[0], Vector3.DOWN), "the sealed room still drops its void faces")

static func _test_see_through_faces_are_not_candidates(t) -> void:
	print("- faces of materials that are not opaque are kept")
	var children := _shell(AABB(Vector3.ZERO, Vector3(256, 128, 256)), 10)
	children.append(_start(50, Vector3(128, 64, 128)))
	children.append(_node(30, Vector3(400, 0, 0), Vector3(432, 32, 32), "brush", "showcase/glass"))
	children.append(_node(31, Vector3(500, 0, 0), Vector3(532, 32, 32)))
	var built := _build(_map(children, { "_cull_void_faces": "0" }))
	var data: FuncGodotData.ParseData = built[0]
	var materials: Dictionary = built[1].texture_materials.duplicate()
	materials.erase("showcase/glass")
	data.entities[0].properties["_cull_void_faces"] = "1"
	var settings: FuncGodotMapSettings = load(SETTINGS)
	TrenchDotVoidCull.apply(data.entities, settings, materials, "glass")
	var glass := data.entities[0].brushes[6]
	var plain := data.entities[0].brushes[7]
	t.check(_shown(glass) == 6, "a brush of a material without an opaque material keeps its faces")
	t.check(_shown(plain) == 0, "an opaque pillar outside the room loses its faces")
