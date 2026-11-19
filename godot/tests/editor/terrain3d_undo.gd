@tool
extends Node
## Checks in a headless Godot editor that TrenchDot edits sent over the live link land in the Terrain3D of the edited
## scene as one undo step, that undo and redo put the regions back and the cache follows each step, and that the
## Terrain3D settings event and the other events behave, and that a scene opened after TrenchDot changed the vertex
## spacing with Godot closed takes it. res://tests/terrain3d_apply_tests.gd starts it with
## godot --headless --editor --path godot res://tests/editor/terrain3d_undo.tscn -- --td-terrain3d-undo
## and it does nothing when the scene is opened any other way.

const FLAG := "--td-terrain3d-undo"
const Apply := preload("res://tests/terrain3d_apply_tests.gd")
const NAME := "editor_undo"
## Inside the project, since the editor opens no scene outside it, and in .godot, which git and the file scan skip.
const SPACING_SCENE := "res://.godot/td_tests/terrain3d_spacing.tscn"

func _ready() -> void:
	if not Engine.is_editor_hint() or not FLAG in OS.get_cmdline_user_args():
		return
	var base := EditorInterface.get_base_control()
	if not base.has_node("Terrain3DUndoRunner"):
		var runner := Runner.new()
		runner.name = "Terrain3DUndoRunner"
		base.add_child.call_deferred(runner)

class Runner extends Node:
	var failures := 0
	var link: TrenchDotEditorIntegration
	var folder := Apply.WORK.path_join(NAME)
	var dir := ProjectSettings.globalize_path(Apply.WORK.path_join(NAME))
	var seq := 0

	func check(cond: bool, what: String) -> void:
		if not cond:
			failures += 1
			printerr("  FAIL: ", what)

	func frames(n: int) -> void:
		for i in n:
			await get_tree().process_frame

	func send(msg: Dictionary) -> Dictionary:
		seq += 1
		msg["seq"] = seq
		var reply: Dictionary = await link.handle_message(JSON.stringify(msg))
		check(reply.get("seq") == seq, "the reply to %s echoes its seq, got %s" % [msg.get("event"), reply])
		return reply

	func hashes() -> Dictionary:
		var out := {}
		for entry in TrenchDotTerrain3DCache.read_manifest(folder).get("regions", []):
			out[Vector2i(int(entry["loc"][0]), int(entry["loc"][1]))] = entry["hash"]
		return out

	func dirty() -> Array:
		var status: Array = link.status()["terrain3d"]
		return status[0]["dirty"] if status.size() == 1 else ["no single Terrain3D"]

	func history(slot: Node) -> UndoRedo:
		var undo := EditorInterface.get_editor_undo_redo()
		return undo.get_history_undo_redo(undo.get_object_history_id(slot))

	func _ready() -> void:
		_run.call_deferred()

	func _run() -> void:
		print("TrenchDot Terrain3D editor undo tests")
		await frames(5)
		for n in get_tree().root.find_children("*", "TrenchDotEditorIntegration", true, false):
			link = n
		if not link or not ClassDB.class_exists("Terrain3D"):
			printerr("  FAIL: the addon's live link or Terrain3D is missing")
			get_tree().quit(1)
			return
		# The test calls the link directly, the port stays free for a Godot editor someone is working in.
		link.stop_live_link()
		var terrain: Node3D = await Apply._world(get_tree(), NAME)
		var slot := TrenchDotTerrain3D.new()
		terrain.get_parent().remove_child(terrain)
		slot.add_child(terrain)
		EditorInterface.get_edited_scene_root().add_child(slot)
		await frames(5)
		await _check_apply_undo_redo(slot, terrain)
		await _check_settings(slot, terrain)
		await _check_look(slot, terrain)
		await _check_other_events()
		slot.queue_free()
		await frames(5)
		await _check_scene_spacing()
		get_tree().quit(1 if failures > 0 else 0)

	func _check_apply_undo_redo(slot: TrenchDotTerrain3D, terrain: Node3D) -> void:
		var data: Object = terrain.get(&"data")
		var west := Vector2i(-1, 0)
		var south := Vector2i(3, -2)
		var gone := Vector2i(0, -1)
		var new := Vector2i(5, 5)
		var status: Array = link.status()["terrain3d"]
		check(status == [{ "dir": dir, "version": "1.1.0", "dirty": [] }], "status lists the Terrain3D of the edited scene, got %s" % [status])
		var before := hashes()
		var west_point := Vector3((west.x * Apply.SIZE + 10) * Apply.SPACING, 0.0, 20 * Apply.SPACING)
		var west_height: float = data.get_height(west_point)
		var flat := Image.create(Apply.SIZE, Apply.SIZE, false, Image.FORMAT_RF)
		Apply._pending(folder, west, { "height": Apply._slanted(west, 4.0), "control": flat }, before[west])
		Apply._pending(folder, gone, { "height": flat, "control": flat }, before[gone], Apply.REMOVE)
		Apply._pending(folder, new, { "height": Apply._slanted(new, -2.0), "control": flat }, Apply.NO_BASE)
		Apply._pending(folder, south, { "height": Apply._slanted(south, 1.0), "control": flat }, Apply.NO_BASE)
		var steps := history(slot).get_current_action()

		var reply := await send({ "event": "terrain3d_apply", "dir": dir, "force": false })
		check(reply.get("ok") == true and reply.get("accepted") == [[-1, 0], [0, -1], [5, 5]], "the apply event applies the regions whose base matches, got %s" % reply)
		check(reply.get("refused") == [{ "loc": [3, -2], "reason": "changed" }], "and refuses the one that changed, got %s" % [reply.get("refused")])
		check(history(slot).get_current_action() == steps + 1 and history(slot).get_current_action_name() == TrenchDotEditorIntegration.TERRAIN3D_EDIT, "it adds one undo step named %s, got %d steps, %s" % [TrenchDotEditorIntegration.TERRAIN3D_EDIT, history(slot).get_current_action() - steps, history(slot).get_current_action_name()])
		var after := hashes()
		check(after.has(new) and not after.has(gone) and after[west] != before[west] and after[south] == before[south], "the cache follows the apply, got %s" % [after])
		check(is_equal_approx(data.get_height(west_point), Apply._slanted(west, 4.0).get_pixel(10, 20).r), "the west region has the sent heights")
		check(dirty() == [], "an applied edit is saved at once, dirty %s" % [dirty()])

		history(slot).undo()
		check(hashes() == before, "an undo puts every region back in the cache, got %s, want %s" % [hashes(), before])
		check(is_equal_approx(data.get_height(west_point), west_height) and data.has_region(gone) and not data.has_region(new), "and in Terrain3D")
		check(dirty() == [[-1, 0], [0, -1]], "the regions an undo changed are unsaved, got %s" % [dirty()])
		var listed: Array = TrenchDotTerrain3DCache.read_manifest(folder).get("dirty", []).map(func(l: Array) -> Array: return [int(l[0]), int(l[1])])
		listed.sort()
		check(listed == [[-1, 0], [0, -1]], "and the manifest lists them as dirty, got %s" % [listed])

		history(slot).redo()
		check(hashes() == after, "a redo puts the applied regions back in the cache, got %s, want %s" % [hashes(), after])
		check(not data.has_region(gone) and data.has_region(new) and dirty() == [[-1, 0], [5, 5]], "and in Terrain3D, dirty %s" % [dirty()])

		steps = history(slot).get_current_action()
		reply = await send({ "event": "terrain3d_apply", "dir": dir })
		check(reply.get("ok") == true and reply.get("accepted") == [] and history(slot).get_current_action() == steps, "an apply with nothing pending adds no undo step, got %s" % reply)
		Apply._pending(folder, south, { "height": Apply._slanted(south, 1.0), "control": flat }, Apply.NO_BASE)
		reply = await send({ "event": "terrain3d_apply", "dir": dir, "force": true })
		check(reply.get("accepted") == [[3, -2]] and reply.get("refused") == [] and hashes()[south] != before[south], "force applies a region that changed, got %s" % reply)
		history(slot).undo()
		check(hashes()[south] == before[south], "and its undo puts it back")
		reply = await send({ "event": "terrain3d_apply", "dir": dir.path_join("elsewhere") })
		check(reply.get("ok") == false and str(reply.get("error")).contains("no open scene"), "another folder is refused, got %s" % reply)
		DirAccess.remove_absolute(TrenchDotTerrain3DCache.region_path(folder, west))
		reply = await send({ "event": "terrain3d_export", "dir": dir + "/", "regions": [[-1, 0]] })
		check(reply.get("ok") == true and reply.get("written") == [[-1, 0]], "the export event exports the regions it names, got %s" % reply)

	func _check_settings(slot: TrenchDotTerrain3D, terrain: Node3D) -> void:
		var steps := history(slot).get_current_action()
		var locs := Array(terrain.get(&"data").get_region_locations()).duplicate()
		locs.sort()
		var before := hashes()
		for bad in [{ "region_size": 100 }, { "vertex_spacing": 0.2499 }, { "vertex_spacing": 100.01 }]:
			var refused := await send(bad.merged({ "event": "terrain3d_settings", "dir": dir }))
			check(refused.get("ok") == false and refused.has("error"), "settings %s are refused, got %s" % [bad, refused])
		check(history(slot).get_current_action() == steps and int(terrain.get(&"region_size")) == Apply.SIZE, "a refused setting changes nothing")
		var reply := await send({ "event": "terrain3d_settings", "dir": dir, "region_size": 128, "vertex_spacing": 100.0 })
		var manifest := TrenchDotTerrain3DCache.read_manifest(folder)
		check(reply == { "ok": true, "region_size": 128, "vertex_spacing": 100.0, "material": "", "assets": "", "ocean_enabled": false, "seq": seq }, "the settings event changes size and spacing, got %s" % reply)
		check(int(manifest.get("region_size", 0)) == 128 and is_equal_approx(manifest.get("vertex_spacing", 0.0), 100.0), "and the cache follows, got %s, %s" % [manifest.get("region_size"), manifest.get("vertex_spacing")])
		check(history(slot).get_current_action() == steps + 1 and history(slot).get_current_action_name() == TrenchDotEditorIntegration.TERRAIN3D_SETTINGS, "as one undo step, got %d steps, %s" % [history(slot).get_current_action() - steps, history(slot).get_current_action_name()])
		var edge := Vector2i(15, 0)
		terrain.get(&"data").add_region_blank(edge, false)
		var crop := await send({ "event": "terrain3d_settings", "dir": dir, "region_size": 64 })
		check(crop.get("ok") == false and str(crop.get("error")).contains("-16 to 15") and int(terrain.get(&"region_size")) == 128, "a region size that would crop the regions is refused, got %s" % crop)
		check(history(slot).get_current_action() == steps + 1, "and adds no undo step")
		terrain.get(&"data").remove_regionl(edge, false)
		history(slot).undo()
		manifest = TrenchDotTerrain3DCache.read_manifest(folder)
		check(int(terrain.get(&"region_size")) == Apply.SIZE and is_equal_approx(terrain.get(&"vertex_spacing"), Apply.SPACING), "its undo puts the settings back")
		check(int(manifest.get("region_size", 0)) == Apply.SIZE and is_equal_approx(manifest.get("vertex_spacing", 0.0), Apply.SPACING), "and the cache follows, got %s, %s" % [manifest.get("region_size"), manifest.get("vertex_spacing")])
		var now := Array(terrain.get(&"data").get_region_locations()).duplicate()
		now.sort()
		check(now == locs and hashes() == before, "with the same regions as before, got %s, want %s" % [now, locs])
		reply = await send({ "event": "terrain3d_settings", "dir": dir, "vertex_spacing": 0.25 })
		check(reply.get("ok") == true and is_equal_approx(terrain.get(&"vertex_spacing"), 0.25), "the smallest spacing is taken, got %s" % reply)
		history(slot).undo()

	func exported() -> Dictionary:
		await get_tree().create_timer(TrenchDotTerrain3D.EXPORT_DELAY + 0.3).timeout
		return TrenchDotTerrain3DCache.read_manifest(folder)

	func _check_look(slot: TrenchDotTerrain3D, terrain: Node3D) -> void:
		var material: Resource = ClassDB.instantiate(&"Terrain3DMaterial")
		material.set(&"world_background", 2)
		var assets: Resource = ClassDB.instantiate(&"Terrain3DAssets")
		var paths := { "material": folder.path_join("look_material.tres"), "assets": folder.path_join("look_assets.tres"), "wrong": folder.path_join("look_wrong.tres") }
		check(ResourceSaver.save(material, paths["material"]) == OK and ResourceSaver.save(assets, paths["assets"]) == OK and ResourceSaver.save(StandardMaterial3D.new(), paths["wrong"]) == OK, "the look files are saved")
		var built_in_material: Resource = terrain.get(&"material")
		var built_in_assets: Resource = terrain.get(&"assets")
		var steps := history(slot).get_current_action()
		var refusals := [
			{ "material": paths["wrong"] },
			{ "assets": paths["material"] },
			{ "material": paths["assets"] },
			{ "material": folder.path_join("missing.tres") },
			{ "assets": 7 },
			{ "ocean_enabled": "yes" },
			{ "material": paths["material"], "region_size": 100 },
		]
		for bad in refusals:
			var refused := await send(bad.merged({ "event": "terrain3d_settings", "dir": dir }))
			check(refused.get("ok") == false and refused.has("error"), "look %s is refused, got %s" % [bad, refused])
		check(history(slot).get_current_action() == steps and terrain.get(&"material") == built_in_material and terrain.get(&"assets") == built_in_assets, "a refused look changes nothing")

		var reply := await send({ "event": "terrain3d_settings", "dir": dir, "material": paths["material"], "assets": paths["assets"], "ocean_enabled": true })
		var want := { "ok": true, "region_size": Apply.SIZE, "vertex_spacing": Apply.SPACING, "material": paths["material"], "assets": paths["assets"], "ocean_enabled": true, "seq": seq }
		check(reply == want, "the settings event picks material, assets and ocean, got %s" % reply)
		check(terrain.get(&"material").resource_path == paths["material"] and terrain.get(&"assets").resource_path == paths["assets"] and terrain.get(&"ocean_enabled") == true, "on the Terrain3D")
		check(history(slot).get_current_action() == steps + 1 and history(slot).get_current_action_name() == TrenchDotEditorIntegration.TERRAIN3D_SETTINGS, "as one undo step, got %d steps" % [history(slot).get_current_action() - steps])
		var manifest := await exported()
		check(manifest.get("resources", {}).get("material") == paths["material"] and manifest.get("resources", {}).get("assets") == paths["assets"], "the cache names the picked files, got %s" % [manifest.get("resources")])
		check(manifest.get("ocean", {}).get("enabled") == true and int(manifest.get("material", {}).get("world_background", -1)) == 2, "with the ocean on and the picked material's background, got %s, %s" % [manifest.get("ocean"), manifest.get("material")])

		reply = await send({ "event": "terrain3d_settings", "dir": dir, "material": paths["material"], "ocean_enabled": true })
		check(reply.get("ok") == true and history(slot).get_current_action() == steps + 1, "picking what the node has adds no undo step, got %s" % reply)

		reply = await send({ "event": "terrain3d_settings", "dir": dir, "material": "" })
		var blank: Resource = terrain.get(&"material")
		check(reply.get("ok") == true and reply.get("material") == "" and reply.get("assets") == paths["assets"], "an empty material clears it, got %s" % reply)
		check(blank != null and blank.resource_path.is_empty() and blank != built_in_material, "and the Terrain3D makes a blank one")
		check(history(slot).get_current_action() == steps + 2, "as one more undo step")
		reply = await send({ "event": "terrain3d_settings", "dir": dir, "material": "" })
		check(history(slot).get_current_action() == steps + 2 and terrain.get(&"material") == blank, "clearing a material no file holds keeps it, got %s" % reply)
		history(slot).undo()
		check(terrain.get(&"material").resource_path == paths["material"], "the undo of a clear puts the picked material back")

		history(slot).undo()
		check(terrain.get(&"material") == built_in_material and terrain.get(&"assets") == built_in_assets and terrain.get(&"ocean_enabled") == false, "the undo of the pick puts the old look back")
		manifest = await exported()
		check(manifest.get("resources", {}).get("material") == "" and manifest.get("ocean", {}).get("enabled") == false, "and the cache follows, got %s, %s" % [manifest.get("resources"), manifest.get("ocean")])

		reply = await send({ "event": "terrain3d_settings", "dir": dir, "region_size": 128, "assets": paths["assets"] })
		check(reply.get("region_size") == 128 and reply.get("assets") == paths["assets"] and history(slot).get_current_action() == steps + 1, "size and look change as one undo step, got %s" % reply)
		history(slot).undo()
		check(int(terrain.get(&"region_size")) == Apply.SIZE and terrain.get(&"assets") == built_in_assets, "and undo together")

	func _check_other_events() -> void:
		var status: Dictionary = link.status()
		check(status.has("maps") and status.has("scene") and status["terrain3d"].size() == 1, "the status keeps its other fields, got %s" % [status.keys()])
		var reply := await send({ "event": "hello" })
		check(reply.get("ok") == true, "hello still answers, got %s" % reply)
		reply = await send({ "event": "terrain3d_sculpt", "dir": dir })
		check(reply.get("ok") == false and reply.get("error") == "unknown event", "an unknown event is still refused, got %s" % reply)

	func _check_scene_spacing() -> void:
		var spaced := Apply.WORK.path_join("editor_spacing")
		var world: Node3D = await Apply._world(get_tree(), "editor_spacing")
		var changed := TrenchDotTerrain3DCache.rewrite_settings(world, spaced, Apply.SIZE, 1.5)
		world.free()
		check(changed.ok, "the folder changes its spacing to 1.5, got %s" % [changed])
		var root := Node3D.new()
		root.name = "SpacingMap"
		var holder := TrenchDotTerrain3D.new()
		holder.name = TrenchDotTerrain3D.NODE_NAME
		root.add_child(holder)
		holder.owner = root
		var old: Node3D = ClassDB.instantiate(&"Terrain3D")
		old.name = "Terrain3D"
		old.set(&"vertex_spacing", Apply.SPACING)
		old.set(&"data_directory", spaced)
		holder.add_child(old)
		old.owner = root
		var pack := PackedScene.new()
		pack.pack(root)
		root.free()
		DirAccess.make_dir_recursive_absolute(SPACING_SCENE.get_base_dir())
		check(ResourceSaver.save(pack, SPACING_SCENE) == OK, "the scene at the old spacing is saved")

		EditorInterface.open_scene_from_path(SPACING_SCENE)
		await frames(5)
		var opened := EditorInterface.get_edited_scene_root()
		var terrain: Node3D = opened.get_node("%s/Terrain3D" % TrenchDotTerrain3D.NODE_NAME) if opened and opened.scene_file_path == SPACING_SCENE else null
		check(terrain != null and is_equal_approx(float(terrain.get(&"vertex_spacing")), 1.5), "the opened scene's Terrain3D takes the spacing, got %s" % [terrain.get(&"vertex_spacing") if terrain else "no scene"])
		check(SPACING_SCENE in EditorInterface.get_unsaved_scenes(), "as an unsaved change, got %s" % [EditorInterface.get_unsaved_scenes()])
		var manifest := TrenchDotTerrain3DCache.read_manifest(spaced)
		check(is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), 1.5) and is_equal_approx(float(manifest.get("scene_vertex_spacing", 0.0)), 1.5), "the cache names the spacing for the scene until it is saved, got %s and %s" % [manifest.get("vertex_spacing"), manifest.get("scene_vertex_spacing")])
		# The headless editor has no viewport texture for the scene thumbnail, so this saves without one.
		EditorInterface.save_scene_as(SPACING_SCENE, false)
		await frames(5)
		manifest = TrenchDotTerrain3DCache.read_manifest(spaced)
		check(not manifest.has("scene_vertex_spacing") and is_equal_approx(float(manifest.get("vertex_spacing", 0.0)), 1.5), "a save drops it, got %s" % [manifest.get("scene_vertex_spacing")])
		check(not SPACING_SCENE in EditorInterface.get_unsaved_scenes() and FileAccess.get_file_as_string(SPACING_SCENE).contains("vertex_spacing = 1.5"), "and the scene file holds the spacing")
		EditorInterface.close_scene()
		await frames(5)
		DirAccess.remove_absolute(SPACING_SCENE)
		DirAccess.remove_absolute(SPACING_SCENE.get_base_dir())
