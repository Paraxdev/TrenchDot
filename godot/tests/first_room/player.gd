extends CharacterBody3D
## First person player for a TrenchDot map. WASD walks, the mouse looks, E uses the door or button you look at,
## Esc frees the mouse and a click captures it again. It is the script of your info_player_start, so the map builds
## the player where the start stands, facing its way.

const SPEED := 5.0
const USE_DISTANCE := 2.5
const LOOK := 0.0025

var head := Node3D.new()
var camera := Camera3D.new()

func _ready() -> void:
	var shape := CollisionShape3D.new()
	shape.shape = CapsuleShape3D.new()
	shape.position.y = 1.0
	add_child(shape)
	head.position.y = 1.6
	head.add_child(camera)
	add_child(head)
	add_to_group(&"player")
	Input.mouse_mode = Input.MOUSE_MODE_CAPTURED

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventMouseMotion and Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		rotate_y(-event.relative.x * LOOK)
		head.rotation.x = clampf(head.rotation.x - event.relative.y * LOOK, -1.5, 1.5)
	elif event is InputEventMouseButton and event.pressed:
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	elif event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_ESCAPE:
			Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
		elif event.keycode == KEY_E:
			_use()

func _physics_process(delta: float) -> void:
	if not is_on_floor():
		velocity += get_gravity() * delta
	var keys := Vector2(
		float(Input.is_key_pressed(KEY_D)) - float(Input.is_key_pressed(KEY_A)),
		float(Input.is_key_pressed(KEY_S)) - float(Input.is_key_pressed(KEY_W)))
	var walk := basis * Vector3(keys.x, 0.0, keys.y).normalized() * SPEED
	velocity = Vector3(walk.x, velocity.y, walk.z)
	move_and_slide()

func _use() -> void:
	var from := camera.global_position
	var query := PhysicsRayQueryParameters3D.create(from, from - camera.global_basis.z * USE_DISTANCE)
	query.exclude = [get_rid()]
	var node: Node = get_world_3d().direct_space_state.intersect_ray(query).get("collider")
	while node and not node.has_method(&"use"):
		node = node.get_parent()
	if node:
		node.use(self)
