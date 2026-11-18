extends RefCounted
## The player script the docs hand out (first_room/player.gd) in a small room with a light, a player start off the
## origin and a func_door, and in rooms with a turned start and with none.
##
## res://tests/run_tests.gd calls [method run], [param t] is that script's instance for its check helpers.

const SETTINGS := "res://demo/demo_map_settings.tres"
const PLAYER := preload("res://tests/first_room/player.gd")
const START := Vector3(-40, 0, -64)

static func run(t) -> void:
	print("- first room player")
	await _test_room(t)
	await _test_turned_start(t)
	await _test_no_start(t)
	Input.mouse_mode = Input.MOUSE_MODE_VISIBLE

static func _entity(id: int, classname: String, origin := Vector3.ZERO, yaw := 0.0, children := []) -> Dictionary:
	return { "type": "entity", "id": id, "classname": classname, "origin": [origin.x, origin.y, origin.z], "angles": [0.0, yaw, 0.0], "properties": {}, "children": children }

static func _room(t, extra: Array) -> Node3D:
	var floor_box: Dictionary = t.box_node(2, Vector3(-256, -32, -192), Vector3(320, 0, 256))
	var map_json := { "format": "trenchdot-map", "properties": {}, "layers": [{ "type": "layer", "id": 1, "children": [floor_box] + extra }] }
	var path := OS.get_temp_dir().path_join("td_first_room_test.tdm")
	FileAccess.open(path, FileAccess.WRITE).store_string(JSON.stringify(map_json))
	var level := Node3D.new()
	t.root.add_child(level)
	var map := FuncGodotMap.new()
	map.map_settings = load(SETTINGS)
	map.global_map_file = path
	level.add_child(map)
	map.build()
	DirAccess.remove_absolute(path)
	return level

static func _spawn(level: Node3D, at: Vector3) -> CharacterBody3D:
	var player := CharacterBody3D.new()
	player.set_script(PLAYER)
	player.position = at
	level.add_child(player)
	return player

static func _key(keycode: Key, pressed: bool) -> InputEventKey:
	var event := InputEventKey.new()
	event.keycode = keycode
	event.physical_keycode = keycode
	event.pressed = pressed
	return event

static func _frames(t, count: int) -> void:
	for i in count:
		await t.physics_frame

static func _test_room(t) -> void:
	var door := _entity(12, "func_door", Vector3.ZERO, 0.0, [t.box_node(13, Vector3(-96, 0, -128), Vector3(48, 96, -112))])
	door["properties"] = { "targetname": "door" }
	var level := _room(t, [_entity(10, "light", Vector3(64, 96, 32)), _entity(11, "info_player_start", START), door])
	var player := _spawn(level, Vector3(3, 5, 2))
	await _frames(t, 90)
	var spot := Vector3(START.x, 0.0, START.z) / 32.0
	t.check(player.is_in_group(&"player"), "the player joins the player group")
	t.check(player.camera.get_parent() == player.head and player.head.get_parent() == player, "the camera sits on the head")
	t.check(absf(player.global_position.x - spot.x) < 0.05 and absf(player.global_position.z - spot.z) < 0.05, "the player starts at the player start, got %s" % player.global_position)
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

	var door_body = t.find_named(level, "entity_door")
	t.check(door_body != null and not door_body.is_open, "the door starts closed")
	if door_body:
		var closed_at: Vector3 = door_body.position
		var reach: float = player.camera.global_position.z - (-112.0 / 32.0)
		t.check(reach > 0.0 and reach <= PLAYER.USE_DISTANCE, "the door is within reach, %s m away" % reach)
		player.global_position = Vector3(START.x, 0.1, START.z) / 32.0
		player.rotation = Vector3.ZERO
		await _frames(t, 2)
		player._unhandled_input(_key(KEY_E, true))
		await _frames(t, 90)
		t.check(door_body.is_open, "E opens the door in front of the player")
		t.check(t.near(door_body.position.y - closed_at.y, 2.0, 0.05), "the door slid up its travel of 2 m, moved %s" % (door_body.position - closed_at))
		player.rotation.y = PI
		player._unhandled_input(_key(KEY_E, true))
		await _frames(t, 5)
		t.check(door_body.is_open, "E facing away from the door does nothing")
	level.queue_free()
	await t.process_frame

static func _test_turned_start(t) -> void:
	var at := Vector3(150, 0, 90)
	var level := _room(t, [_entity(20, "info_player_start", at, 90.0)])
	var player := _spawn(level, Vector3(3, 5, 2))
	await _frames(t, 60)
	var start := PLAYER.find_start(level)
	var spot := at / 32.0
	t.check(absf(player.global_position.x - spot.x) < 0.05 and absf(player.global_position.z - spot.z) < 0.05, "a second room puts the player at its own start, not the first one's, got %s" % player.global_position)
	t.check(start != null and absf(wrapf(player.global_rotation.y - start.global_rotation.y, -PI, PI)) < 0.01, "the player faces the way the start does")
	t.check(absf(player.global_rotation.y) > 0.1, "the start is turned, so the player turns with it")
	level.queue_free()
	await t.process_frame

static func _test_no_start(t) -> void:
	var level := _room(t, [_entity(30, "light", Vector3(0, 96, 0))])
	var player := _spawn(level, Vector3(3, 5, 2))
	await _frames(t, 1)
	t.check(is_equal_approx(player.global_position.x, 3.0) and is_equal_approx(player.global_position.z, 2.0) and player.global_position.y > 4.5, "without a player start the player stays where it was placed, got %s" % player.global_position)
	await _frames(t, 90)
	t.check(is_equal_approx(player.global_position.x, 3.0) and is_equal_approx(player.global_position.z, 2.0), "and still falls straight down onto the floor")
	t.check(player.is_on_floor(), "and lands on it")
	level.queue_free()
	await t.process_frame
