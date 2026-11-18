extends Node3D
## A small playable cutscene built from res://demo/maps/scripted_scene.tdm. Walk forward into the trigger and the
## scene plays itself through entity I/O: a guide spawns and walks a path, text appears, an explosive barrel goes off
## and hurts you, a light switches on and the exit opens. WASD to move, mouse to look, F3 toggles the I/O overlay.

const MAP := "res://demo/maps/scripted_scene.tdm"
const SETTINGS := "res://demo/demo_map_settings.tres"

func _ready() -> void:
	add_child(TrenchDotDebugOverlay.new())
	var map := FuncGodotMap.new()
	map.map_settings = load(SETTINGS)
	map.local_map_file = MAP
	add_child(map)
	map.build()
