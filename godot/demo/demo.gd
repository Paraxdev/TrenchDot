extends Node3D
## Plays the built showcase maps with a first person player. The number keys switch maps, F3 toggles the I/O debug overlay.
## Build the maps first: godot --headless --path godot --script res://tests/build_showcase.gd

const MAPS := ["res://demo/showcase/mountain_house.scn", "res://demo/showcase/church_school.scn", "res://demo/showcase/lighthouse_forest.scn", "res://demo/showcase/night_district.scn", "res://demo/showcase/withered_city.scn"]

@export var start_map := 2

var _level: Node

func _ready() -> void:
	add_child(TrenchDotDebugOverlay.new())
	load_map(start_map)

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo and event.keycode >= KEY_1 and event.keycode < KEY_1 + MAPS.size():
		load_map(event.keycode - KEY_1)

func load_map(index: int) -> void:
	if _level:
		_level.queue_free()
	var scene := load(MAPS[index]) as PackedScene
	if not scene:
		push_error(load_problem(MAPS[index], TrenchDotEntityPack.installed()))
		return
	_level = scene.instantiate()
	add_child(_level)

## Why the showcase scene at [param path] did not load. The scenes use the scripts of the Gameplay entities pack.
static func load_problem(path: String, pack_installed: bool) -> String:
	if not FileAccess.file_exists(path):
		return "%s is missing, run tests/build_showcase.gd first" % path
	if not pack_installed:
		return "%s needs the Gameplay entities pack in %s. Install it with Gameplay > Install Gameplay Entities in TrenchDot" % [path, TrenchDotEntityPack.DIR]
	return "%s could not be loaded, see the errors above or run tests/build_showcase.gd to build it again" % path
