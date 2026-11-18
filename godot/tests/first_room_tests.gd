extends RefCounted
## The first person player the docs hand out (first_room/player.gd), as the project's override of info_player_start
## next to an overridden func_button, in a small room with a light, a player start off the origin and the pack's
## func_door, and in rooms with a turned start and with none. The overrides are the fixture in first_room/project,
## written by the editor's Override in project.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const FIXTURE := "res://tests/first_room/project/entities"
const SETTINGS := FIXTURE + "/project_map_settings.tres"
const DEMO_SETTINGS := "res://demo/demo_map_settings.tres"
const START := Vector3(-40, 0, -64)

static func run(t) -> void:
	print("- first room player")
	_test_fixture(t)
	await _test_room(t)
	await _test_turned_start(t)
	await _test_no_start(t)
	Input.mouse_mode = Input.MOUSE_MODE_VISIBLE

static func _entity(id: int, classname: String, origin := Vector3.ZERO, yaw := 0.0, children := []) -> Dictionary:
	return { "type": "entity", "id": id, "classname": classname, "origin": [origin.x, origin.y, origin.z], "angles": [0.0, yaw, 0.0], "properties": {}, "children": children }

static func _room(t, extra: Array, settings := SETTINGS) -> Node3D:
	var floor_box: Dictionary = t.box_node(2, Vector3(-256, -32, -192), Vector3(320, 0, 256))
	var map_json := { "format": "trenchdot-map", "properties": {}, "layers": [{ "type": "layer", "id": 1, "children": [floor_box] + extra }] }
	var path := OS.get_temp_dir().path_join("td_first_room_test.tdm")
	FileAccess.open(path, FileAccess.WRITE).store_string(JSON.stringify(map_json))
	var level := Node3D.new()
	t.root.add_child(level)
	var map := FuncGodotMap.new()
	var map_settings: FuncGodotMapSettings = load(settings).duplicate()
	map_settings.base_texture_dir = "res://demo/textures"
	map.map_settings = map_settings
	map.global_map_file = path
	level.add_child(map)
	map.build()
	DirAccess.remove_absolute(path)
	return level

static func _built(t, level: Node, classname: String) -> Array[Node]:
	return t.collect(level, func(n: Node) -> bool: return String(n.name).ends_with("_" + classname))

static func _players(t) -> Array[Node]:
	return t.collect(t.root, func(n: Node) -> bool: return n.is_in_group(&"player"))

static func _key(keycode: Key, pressed: bool) -> InputEventKey:
	var event := InputEventKey.new()
	event.keycode = keycode
	event.physical_keycode = keycode
	event.pressed = pressed
	return event

static func _frames(t, count: int) -> void:
	for i in count:
		await t.physics_frame

static func _test_fixture(t) -> void:
	var config: TrenchDotGameConfig = load(FIXTURE + "/project_game_config.tres")
	config.csharp_source_dirs = []
	var clashes := {}
	for clash in config.build_config().get("clashes", []):
		clashes[clash["classname"]] = [clash.get("intended", false), clash["definitions"][-1]]
	var want := {
		"info_player_start": [true, FIXTURE + "/info_player_start.tres"],
		"func_button": [true, FIXTURE + "/func_button.tres"],
	}
	t.check(clashes == want, "the fixture overrides the player start and func_button on purpose, got %s" % [clashes])
	t.check(config.map_settings.resource_path == SETTINGS and config.map_settings.entity_fgd == config.fgd_file, "the fixture's game config builds with its map settings and FGD")

static func _test_room(t) -> void:
	var door := _entity(12, "func_door", Vector3.ZERO, 0.0, [t.box_node(13, Vector3(-96, 0, -128), Vector3(48, 96, -112))])
	var button := _entity(14, "func_button", Vector3.ZERO, 0.0, [t.box_node(15, Vector3(160, 0, 96), Vector3(176, 16, 120))])
	var level := _room(t, [_entity(10, "light", Vector3(64, 96, 32)), _entity(11, "info_player_start", START), door, button])
	await _frames(t, 60)
	var starts := _built(t, level, "info_player_start")
	t.check(starts.size() == 1, "the room builds one player start, got %s" % [starts])
	if starts.size() != 1:
		level.queue_free()
		await t.process_frame
		return
	var player: CharacterBody3D = starts[0] as CharacterBody3D
	t.check(player != null and player.get_script().resource_path == FIXTURE + "/info_player_start.gd", "the player start is a CharacterBody3D with the project's player script")
	if player == null:
		level.queue_free()
		await t.process_frame
		return
	t.check(_players(t) == [player], "the player start is the only player, got %s" % [_players(t)])
	t.check(player.camera.get_parent() == player.head and player.head.get_parent() == player, "the camera sits on the head")
	var spot := START / 32.0
	t.check(absf(player.global_position.x - spot.x) < 0.05 and absf(player.global_position.z - spot.z) < 0.05, "the player stands where the start is, got %s" % player.global_position)
	t.check(player.is_on_floor() and absf(player.global_position.y) < 0.15, "the player stands on the floor, y is %s" % player.global_position.y)
	t.check(player.camera.global_position.y > 1.4, "the camera is at head height")

	var before: Vector3 = player.global_position
	Input.parse_input_event(_key(KEY_W, true))
	await _frames(t, 30)
	Input.parse_input_event(_key(KEY_W, false))
	await _frames(t, 2)
	var walked: Vector3 = player.global_position - before
	t.check(walked.z < -0.5 and absf(walked.x) < 0.01, "W walks forward, moved %s" % walked)
	var stopped: Vector3 = player.global_position
	await _frames(t, 10)
	t.check(player.global_position.distance_to(stopped) < 0.01, "the player stops when the key is released")

	var doors: Array[Node] = t.collect(level, func(n: Node) -> bool: return n is TDDoor)
	t.check(doors.size() == 1 and not doors[0].is_open, "the pack's func_door builds closed, got %s" % [doors])
	if doors.size() == 1:
		var door_body: TDDoor = doors[0]
		var closed_at: Vector3 = door_body.position
		player.global_position = Vector3(START.x, 0.1, START.z) / 32.0
		player.rotation = Vector3.ZERO
		await _frames(t, 2)
		var reach: float = player.camera.global_position.z - (-112.0 / 32.0)
		t.check(reach > 0.0 and reach <= player.USE_DISTANCE, "the door is within reach, %s m away" % reach)
		player._unhandled_input(_key(KEY_E, true))
		await _frames(t, 90)
		t.check(door_body.is_open, "E opens the door in front of the player")
		t.check(t.near(door_body.position.y - closed_at.y, 2.0, 0.05), "the door slid up its travel of 2 m, moved %s" % (door_body.position - closed_at))
		player.rotation.y = PI
		player._unhandled_input(_key(KEY_E, true))
		await _frames(t, 5)
		t.check(door_body.is_open, "E facing away from the door does nothing")

	var buttons := _built(t, level, "func_button")
	t.check(buttons.size() == 1 and buttons[0] is TDButton and buttons[0].get_script().resource_path == FIXTURE + "/func_button.gd", "func_button builds as a TDButton with the project's script, got %s" % [buttons])
	if buttons.size() == 1 and buttons[0] is TDButton:
		var pressed: TDButton = buttons[0]
		var rest: Vector3 = pressed.position
		pressed.use(player)
		await _frames(t, 15)
		t.check(pressed.is_pressed and t.near(pressed.position - rest, Vector3(0, -2, 0) / 32.0, 0.005), "the overridden button still presses in by its travel, moved %s" % (pressed.position - rest))
	level.queue_free()
	await t.process_frame

static func _test_turned_start(t) -> void:
	var at := Vector3(150, 0, 90)
	var marked := _room(t, [_entity(20, "info_player_start", at, 90.0)], DEMO_SETTINGS)
	var markers := _built(t, marked, "info_player_start")
	var facing: float = markers[0].global_rotation.y if markers.size() == 1 and markers[0] is Marker3D else NAN
	t.check(absf(facing) > 0.1, "the addon's own player start is a turned Marker3D, faces %s" % facing)
	marked.queue_free()
	await t.process_frame

	var level := _room(t, [_entity(20, "info_player_start", at, 90.0)])
	await _frames(t, 60)
	var players := _players(t)
	t.check(players.size() == 1 and level.is_ancestor_of(players[0]), "a second room has its own player and none of the first room's, got %s" % [players])
	if players.size() == 1:
		var player: Node3D = players[0]
		var spot := at / 32.0
		t.check(absf(player.global_position.x - spot.x) < 0.05 and absf(player.global_position.z - spot.z) < 0.05, "the player stands at this room's start, got %s" % player.global_position)
		t.check(absf(wrapf(player.global_rotation.y - facing, -PI, PI)) < 0.01, "the player faces the way the start does, %s against %s" % [player.global_rotation.y, facing])
	level.queue_free()
	await t.process_frame

static func _test_no_start(t) -> void:
	var level := _room(t, [_entity(30, "light", Vector3(0, 96, 0))])
	await _frames(t, 5)
	t.check(_players(t).is_empty(), "a room without a player start has no player, got %s" % [_players(t)])
	level.queue_free()
	await t.process_frame
