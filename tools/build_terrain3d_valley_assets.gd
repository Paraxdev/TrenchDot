extends SceneTree
## Builds the ground textures of the Terrain3D valley showcase from the showcase photo textures, and refreshes the
## TrenchDot cache of the valley's data folder so its texture list and thumbnails are current. Terrain3D wants one size
## and format per texture array, with the height in the albedo alpha and the roughness in the normal alpha.
## From the repository root, the first run writes the PNGs, the second one builds the assets after Godot imported them:
##   godot --headless --path godot --script <repo>/tools/build_terrain3d_valley_assets.gd
##   godot --headless --path godot --import
## Then set mipmaps/generate=true in the .import file of each PNG and import once more, Terrain3D warns without them.
##   godot --headless --path godot --script <repo>/tools/build_terrain3d_valley_assets.gd

const FOLDER := "res://terrain3d_valley"
const SOURCE := "res://demo/textures/showcase/"
const SIZE := 512
const ROUGHNESS := 0.8
## name, tiles per meter
const GROUND := [["grass", 0.5], ["cliff", 0.33], ["dirt", 0.5], ["sand", 0.66]]

func _initialize() -> void:
	DirAccess.make_dir_recursive_absolute(FOLDER.path_join("textures"))
	var missing := false
	for entry in GROUND:
		for suffix in ["albedo", "normal"]:
			missing = missing or not ResourceLoader.exists(_png(entry[0], suffix))
	if missing:
		for entry in GROUND:
			_write_pngs(entry[0])
		print("wrote the PNGs, import the project and run this script again")
		quit(0)
		return
	_build.call_deferred()

static func _png(name: String, suffix: String) -> String:
	return "%s/textures/%s_%s.png" % [FOLDER, name, suffix]

func _write_pngs(name: String) -> void:
	var albedo := Image.load_from_file(ProjectSettings.globalize_path(SOURCE + name + ".jpg"))
	albedo.convert(Image.FORMAT_RGBA8)
	albedo.resize(SIZE, SIZE, Image.INTERPOLATE_LANCZOS)
	for y in SIZE:
		for x in SIZE:
			var c := albedo.get_pixel(x, y)
			albedo.set_pixel(x, y, Color(c.r, c.g, c.b, c.get_luminance()))
	albedo.save_png(ProjectSettings.globalize_path(_png(name, "albedo")))
	var normal := Image.load_from_file(ProjectSettings.globalize_path(SOURCE + name + "_normal.jpg"))
	normal.convert(Image.FORMAT_RGBA8)
	normal.resize(SIZE, SIZE, Image.INTERPOLATE_LANCZOS)
	for y in SIZE:
		for x in SIZE:
			var c := normal.get_pixel(x, y)
			normal.set_pixel(x, y, Color(c.r, c.g, c.b, ROUGHNESS))
	normal.save_png(ProjectSettings.globalize_path(_png(name, "normal")))

func _build() -> void:
	var assets: Resource = ClassDB.instantiate(&"Terrain3DAssets")
	for i in GROUND.size():
		var asset: Resource = ClassDB.instantiate(&"Terrain3DTextureAsset")
		asset.set(&"name", str(GROUND[i][0]).capitalize())
		asset.set(&"albedo_texture", load(_png(GROUND[i][0], "albedo")))
		asset.set(&"normal_texture", load(_png(GROUND[i][0], "normal")))
		asset.set(&"uv_scale", GROUND[i][1])
		assets.call(&"set_texture_asset", i, asset)
	var path := FOLDER.path_join("terrain3d_assets.tres")
	var err := ResourceSaver.save(assets, path)
	print("assets saved: %s" % error_string(err))
	var terrain := ClassDB.instantiate(&"Terrain3D") as Node3D
	terrain.set(&"data_directory", FOLDER)
	terrain.set(&"assets", assets)
	root.add_child(terrain)
	await process_frame
	var result := TrenchDotTerrain3DCache.export(terrain, FOLDER, null, { "editor_open": false, "scene_values_from_cache": true })
	terrain.free()
	print("cache export ok %s, written %s, problems %s" % [result.ok, result.written, result.problems])
	quit(0 if err == OK and result.ok else 1)
