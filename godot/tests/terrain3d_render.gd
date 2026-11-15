extends SceneTree
## Renders the Terrain3D valley showcase scene with the renderer picked on the command line and checks that the
## terrain, its textures and the scatter show up. It needs a window and a GPU (or xvfb with software Vulkan), headless
## renders nothing. Run it once per renderer:
## godot --path godot --rendering-method forward_plus --script res://tests/terrain3d_render.gd [-- <screenshot png>]
## godot --path godot --rendering-method mobile --script res://tests/terrain3d_render.gd
## godot --path godot --rendering-method gl_compatibility --script res://tests/terrain3d_render.gd

const SCENE := "res://demo/showcase/terrain3d_valley.scn"
const REGIONS := 6
const TEXTURES := 4

# Positions in meters, looking down the valley from its south west corner.
const EYE := Vector3(40.0, 0.0, 230.0)
const TARGET := Vector3(300.0, 0.0, 90.0)
const EYE_ABOVE := 45.0

var failures: Array[String] = []

func _initialize() -> void:
	_run.call_deferred()

func _fail(message: String) -> void:
	failures.append(message)
	printerr("FAIL ", message)

func _run() -> void:
	var method := RenderingServer.get_current_rendering_method()
	if not ClassDB.class_exists(&"Terrain3D"):
		print("terrain3d_render: skipped, Terrain3D is not installed (python3 tools/fetch_terrain3d.py)")
		quit(0)
		return
	if DisplayServer.get_name() == "headless":
		print("terrain3d_render: skipped, headless renders nothing, run it with a window or xvfb")
		quit(0)
		return
	root.size = Vector2i(1280, 720)
	var instance: Node = (load(SCENE) as PackedScene).instantiate()
	root.add_child(instance)
	var camera := Camera3D.new()
	camera.far = 1200.0
	camera.fov = 70.0
	instance.add_child(camera)
	camera.make_current()
	for i in 10:
		await process_frame

	var slots := instance.find_children("*", "TrenchDotTerrain3D", true, false)
	if slots.size() != 1:
		_fail("expected 1 TrenchDotTerrain3D slot, found %d" % slots.size())
	else:
		var terrain: Object = slots[0].get_terrain()
		if terrain == null:
			_fail("the slot holds no Terrain3D node")
		else:
			var regions: int = terrain.data.get_regions_all().size()
			if regions != REGIONS:
				_fail("expected %d Terrain3D regions, found %d" % [REGIONS, regions])
			if terrain.assets == null:
				_fail("the Terrain3D node has no assets resource")
			else:
				var textures: int = terrain.assets.get_texture_count()
				if textures != TEXTURES:
					_fail("expected %d Terrain3D textures, found %d" % [TEXTURES, textures])
			var eye := EYE
			var target := TARGET
			eye.y = terrain.data.get_height(eye) + EYE_ABOVE
			target.y = terrain.data.get_height(target)
			camera.look_at_from_position(eye, target)

	var scatter_instances := 0
	for node in instance.find_children("*", "MultiMeshInstance3D", true, false):
		var count: int = node.multimesh.instance_count if node.multimesh else 0
		if count > 0:
			scatter_instances += 1
	if scatter_instances < 2:
		_fail("expected scatter MultiMeshes with instances, found %d" % scatter_instances)

	for i in 40:
		await process_frame
	var image := root.get_viewport().get_texture().get_image()
	if image == null or image.is_empty():
		_fail("the viewport gave no image")
	else:
		var distinct := {}
		for y in range(0, image.get_height(), 12):
			for x in range(0, image.get_width(), 12):
				distinct[image.get_pixel(x, y).to_html(false)] = true
		if distinct.size() < 64:
			_fail("the frame looks flat, %d distinct colors sampled" % distinct.size())
		var args := OS.get_cmdline_user_args()
		if args.size() > 0:
			print("saved ", args[0], " ", error_string(image.save_png(args[0])))
		print("terrain3d_render: %s, %d sampled colors, %d scatter MultiMeshes" % [method, distinct.size(), scatter_instances])

	instance.queue_free()
	await process_frame
	print("terrain3d_render %s: %d failures" % [method, failures.size()])
	quit(1 if failures.size() > 0 else 0)
