# MCP server

The editor has an [MCP](https://modelcontextprotocol.io) server built in. An agent like Claude Code, or a plain Python
script, can use it to inspect and edit maps, take screenshots and play input into the UI.

Use it to let an AI assistant build or review a level, to script repetitive edits, or to test the editor
automatically. The server is off until you start it, and it only listens on your own machine.

## Starting it

| Transport | How to start it |
| --- | --- |
| HTTP | Launch the editor with `--mcp-http` for port 7841, or `--mcp-http=PORT` for another port. To have it start every time, go to *File > Preferences*, open *Advanced*, tick *enable on startup* next to *MCP server (HTTP, localhost)*, set the port beside it and restart |
| stdio | Launch with `--mcp`. The client starts the editor itself for each session and talks to it over standard input and output |

Pick HTTP when you keep the editor open and work in it yourself while an agent helps. Pick stdio when the client
should own its editor, or when other people use the same computer. To connect Claude Code to either:

```sh
claude mcp add --transport http trenchdot http://127.0.0.1:7841/mcp   # HTTP
claude mcp add trenchdot -- /path/to/trenchdot --mcp                 # stdio
```

The repository's `.mcp.json` already points Claude Code at the HTTP server. *File > Preferences* shows the command
for the server that runs, and *Copy command* puts it on the clipboard.

Started the editor on another port? Set `TRENCHDOT_MCP_URL` and
[tools/mcp_client.py](https://github.com/Paraxdev/TrenchDot/blob/main/tools/mcp_client.py) and
[tools/mcp_script.py](https://github.com/Paraxdev/TrenchDot/blob/main/tools/mcp_script.py) connect there instead.

### The HTTP token

Any program on your computer can reach the HTTP server, and so can other people signed in to it. To keep them out,
tick *Require a token* under *Advanced* in *File > Preferences*. From the next request on, the server only answers
requests that send its token as `Authorization: Bearer <token>`. The command in Preferences then includes the token,
use it instead of `.mcp.json`. stdio needs no token, only the client that started the editor can talk to it.

The editor makes a random token the first time you ask for one and keeps it in `mcp-token`, a file only you can read,
next to its settings: `%APPDATA%\TrenchDot\data` on Windows, `~/.local/share/trenchdot` on Linux and
`~/Library/Application Support/TrenchDot` on macOS. A `--default-prefs` run keeps its own in
`trenchdot-default-prefs.mcp-token` in the temp folder.

To pick the token yourself for one run, set `TRENCHDOT_MCP_TOKEN` or launch with `--mcp-token=<token>`, which wins
over the variable. Either one turns the token on whatever Preferences says, and *Require a token* shows it ticked and
greyed out. `tools/mcp_client.py` and `tools/mcp_script.py` send a token only when you give them one, with `--token` or
`TRENCHDOT_MCP_TOKEN`.

## Turning MCP off

To keep agents and scripts out of the editor altogether, untick *MCP* at the top of the MCP part of
*File > Preferences*. The HTTP server closes at once and drops its open connections, a client on stdio gets an error
for every request, and the other MCP settings grey out. The setting is saved at once, so every editor you start
without `--default-prefs` keeps MCP off until you tick it again, which brings the HTTP server back on its port.

![The MCP part of Preferences: MCP ticked, the HTTP server row with its port, and Require a token](assets/mcp/prefs-mcp.png)

To turn MCP off for one run whatever Preferences says, set `TRENCHDOT_DISABLE_MCP` to anything but empty or `0`.
*MCP* then shows unticked and greyed out. While MCP is off, `--mcp` and `--mcp-http` start no server. The editor still
opens and its status line says MCP is off, and a client on stdio gets an error saying so instead of waiting.

An agent can turn MCP off as well, with `set_editor` and `"mcp": false`. Only you can turn it back on.

## Tools

| Area | What it is for | Tools |
| --- | --- | --- |
| Inspect | Read the map, the editor state, the entity definitions and how to use an entity from game code, without changing anything | `get_state`, `summarize_map`, `list_nodes`, `get_node`, `changes_since`, `validate_map`, `get_game_config`, `code_reference` |
| Create | Add brushes, meshes, terrains, entities and imported models, and trim along face edges | `create_brush`, `create_mesh`, `create_terrain`, `create_entity`, `import_model`, `trim` |
| Edit | Change what exists: entity keys, selection, placement, faces, textures, terrain, layers and groups, and worldspawn | `update_entity`, `select`, `transform`, `duplicate`, `set_face`, `mesh_edit`, `texture`, `terrain_edit`, `hierarchy`, `set_map_properties`, `bake_lighting` |
| Paint and gameplay | Scatter models, blend materials, paste pieces from the prefab library, and turn brushes into doors, platforms and buttons | `scatter`, `blend`, `prefab_library`, `random_fill`, `gameplay` |
| Editor | Run menu actions, open and save files, change editor settings, move the camera, take screenshots, send input and check where a player can walk | `run_action`, `map_file`, `open_project`, `set_editor`, `set_camera`, `screenshot`, `simulate_input`, `walkability` |
| Project | Add the Gameplay entities pack, the nature models or the demo to the open Godot project, and override an entity or add the first person player | `project_content`, `override_entity` |
| Scripts | Replay a saved list of tool calls, see [MCP scripts](mcp-scripts.md) | `run_script` |

Each tool's description lists its operations and arguments, so an agent learns them straight from the server.
[tools/mcp_client.py](https://github.com/Paraxdev/TrenchDot/blob/main/tools/mcp_client.py) prints them with `list`
and calls a tool from the shell:

```sh
python tools/mcp_client.py call screenshot '{"target":"3d","width":1280,"height":720}' --out shot.png
```

## Screenshots

`screenshot` returns a PNG of the whole editor window, or of one view when `target` is `3d`, `top`, `front` or `side`.
For a view, the other arguments decide what you get:

| Arguments | Result |
| --- | --- |
| None | The whole editor window |
| `target` only | That docked view as it is, overlays included |
| `width` and `height` | Rendered offscreen up to 4096 pixels, without grid, entity boxes, gizmos or selection |
| `width`, `height` and `"overlays": true` | Offscreen, with the editor overlays |
| `"overlays": false` | A clean shot at the docked size |

### What Godot renders

`"source": "godot"` asks the Godot editor to render the map with its real lights, environment, fog and post
processing. You get that PNG back, plus any errors or warnings Godot logged while building it. Handy for judging
lighting and materials without launching the game.

```sh
python tools/mcp_client.py call screenshot '{"source":"godot","position":[0,256,512],"look_at":[0,64,0]}' --out godot.png
```

1. Save the map inside the project that is open in TrenchDot.
2. Open that project in the Godot editor, with a scene whose `FuncGodotMap` builds the map.
3. Call `screenshot` with `"source": "godot"`. Without `position` and `look_at` it uses the 3D view's camera.

Without [live mode](godot/live.md) Godot first builds the map as shown in TrenchDot, like *Build in Godot*.
`"build": false` captures the scene as Godot has it. `width` and `height` default to 1280 by 720, and `fov` to the 3D
view's.

> [!NOTE]
> Godot needs a window to render, a headless Godot returns an error instead of an image. The window can be
> in the background or minimized.

## Reading and reviewing a map

| Step | Tool |
| --- | --- |
| Get an overview | `summarize_map` lists the layers, entity classes, I/O links and materials, and any missing assets |
| Find nodes | `list_nodes` filters by `layer`, `material`, `property` (`{"model": "*crate*"}`) or a `box` in the map |
| Look at one | `get_node` returns one node. `compact: true` leaves out vertices and faces |
| Show what you changed | `changes_since` compares against the last save, a number of `undo_steps` back, or a map `file` |
| Take it back | Each call and each `run_script` is one undo step named `MCP: ...`. `run_action undo` with `steps` undoes several |

See [Reviewing changes](editor/reviewing.md) for keeping maps in git.

## Behavior worth knowing

| Topic | Behavior |
| --- | --- |
| Browsers | The HTTP server refuses any request with an `Origin` header and any POST that is not `application/json`, so web pages cannot reach it, not even ones served from localhost |
| Errors | A tool that could not do its job returns `isError`, including a failed `run_action` |
| File dialogs | Actions that open one are refused, use `map_file` with a path. `run_action save` needs a map that already has a file |
| Unsaved tabs | `run_action close_tab` refuses unsaved changes unless `args.discard` is true. `args.close_others` closes every tab but the active one instead |
| File paths | `res://` and relative paths in `map_file`, `run_script`, `import_model`, `changes_since` and `validate_map` start at the open project, or at the editor's working directory without one. `map_file open` and `open_tab` resolve the path to an absolute one before opening, so a later `save` cannot land somewhere else |
| Editor settings | `transform` and `run_action rotate` and `flip` pivot on the selection center, snapped to the grid only while snapping is on, and keep textures locked while UV lock is on. `transform` takes `snap` and `uv_lock` to override that for one call, `duplicate` takes `uv_lock`. `run_action sculpt` and `blend` use the tool settings a person can change, including symmetry, `terrain_edit` uses fixed defaults |
| Selection | `select` refuses unknown ids and skips hidden or locked nodes, listing them in `skipped` |
| CSG | `csg_subtract`, `csg_merge`, `csg_intersect`, `csg_hollow` and `clip_apply` return `replaced`, old ids mapped to new ones |
| Timeouts | Calls time out after 120 s, except `run_script`, `project_content` and `bake_lighting` |
| Content question | The question a new project gets about ready made content closes on the first call other than `get_state`, `screenshot` and `simulate_input`, and does not open again in that session |
| Script variables | `$name/rest` (unbraced) is an error when `name` is a known variable, since only `$name` alone or `${name}/rest` expand it. Write `${project}/maps` |

## Tool notes

| Tool | Note |
| --- | --- |
| `run_action` `copy`, `cut`, `paste` | Copy and cut return the clipboard text, paste takes it as `args.text` with an optional `offset` or `origin` |
| `run_action` `align`, `distribute` | `align` takes `args.axis` and `args.to` (`min`, `center` or `max`), `distribute` takes `args.axis`. Both treat each selected group, brush entity or object as one unit and do not snap |
| `run_action` `clip_apply` | Takes a plane (`point` and `normal`, or three `points`) and `keep`: `front`, `back` or `both` |
| `run_action` `create_decal` | Places a [decal](editor/decals.md), use it instead of thin brushes |
| `create_brush` | `openings` carves doors and windows, `{min, max, count, step, rows, row_step}` repeats one as a grid |
| `create_mesh` `rotate` | `[x, y, z]` degrees in Godot's YXZ order, so `[0, 0, 90]` lays a cylinder along X |
| `transform` | Applies translate, rotate, flip, scale, scale_to and shear in that order, as one undo step. `rotate` turns 90 degrees about Y unless told otherwise, `scale` multiplies the size about the selection center by `[x, y, z]` factors (a negative factor mirrors, 0 is refused), `shear` slides one side of the selection's bounds, for example `{"side": "+y", "along": "x", "amount": 32}` |
| `create_terrain` | `origin` is the terrain's minimum corner, unlike other tools. `center` places it around a point instead |
| `create_brush`, `create_mesh` | A gable ridge runs along Z for brushes and along X for meshes. Both tools take `ridge_x` and `ridge_z` |
| `map_file` exports | `path` is the written file, kept for older callers, `exported` names it too and `map_path` is the open map |
| `terrain_edit` | `probe [x, z]` without `op` only reads the height. `clear_layer` needs `layer`. `op: "resize"` grows or crops the terrain by whole cells per side, `north`, `south`, `west` and `east` (positive adds, negative removes), or to `cells: [x, z]` around the center with the odd cell going east or south, and `cell_size` resamples it over the same area. The sides apply first, then the cell size, and `cells` cannot be combined with `cell_size`. `fill` is `edge` (default, new ground repeats the edge) or `zero`, and `scatter` is `drop` (default, removes scatter on the terrain over removed ground) or `keep`. `dry_run: true` returns the plan without changing anything, and a resize that changes nothing returns `changed: false` and records no undo step. A terrain has 1 to 2048 cells along each axis and a cell size from 0.001 to 100000, anything outside is refused whole |
| `symmetry` | `run_action` `sculpt`, `blend` and `terrain_edit` take it and `set_editor` sets the one the Sculpt and Blend tool bars use, `get_state` reports it as `editor.symmetry`. It is `off`, `x`, `z` or `xz` (planes through each terrain's center), or an object `{axes, center, x, z}` where `center` is `terrain`, `world` or `custom` and `x` and `z` are the world positions of the planes with a custom center. A value replaces the whole setting. `sculpt` and `blend` follow the bar when it is missing, `terrain_edit` is off, so scripts replay the same. `terrain_edit` takes it for `sculpt`, `sculpt_path`, `paint_path` and `holes` by `center`, and refuses it on any other op |
| `run_action` `mirror_linked` | Makes a [linked copy](editor/brushes.md#mirror) of the selection mirrored across a plane. `args.axis` is `x` (default), `y` or `z`, `args.plane` is `center`, `max`, `min`, `world` or `custom` and `args.at` is the position for `custom`, alone it implies `custom`. Left out, the plane is the one the Inspector holds, and the call does not change that setting. `args.gap` (0 by default, in map units) moves the copy that much further out, up to 65536 (more is an error), a negative one down to minus the narrower half's width, below that it stops there. The reply has `copies` (the new groups, now selected), `plane` as `{axis, at}` and `gap`, the value applied after that limit. Nothing selected is an error |
| `run_action` `set_mirror_gap` | Sets the distance between the two selected halves of a mirrored pair, like the Gap field in the Inspector. `args.gap` is in map units and both halves move evenly, so the plane stays. `args.axis` picks one plane when the selection has several. Below minus the narrower half's width it stops there. The reply has `plane` and `gap`, the value applied. Nothing mirrored selected, no plane on that axis or a gap over 65536 is an error |
| `run_action` `mirror_again` | Moves the partner of a selected half that was slid along its plane back across from it, one undo step. The reply has `moved` (how many groups), `distance` and `plane`. Nothing selected that was slid along its plane is an error |
| `run_action` `select_linked` | Selects every copy of the selected linked groups, or of the linked group around a selected object, leaving hidden and locked copies out. Run it on one half to move both |
| `run_action` `separate_linked_groups` | Makes the selected linked groups plain groups, and frees a copy left alone in its set. `unlink_groups` is the old name and still works |
| `run_action` `apply_mirror` | Turns a mirrored linked pair into plain geometry like [Apply Mirror](editor/brushes.md#mirror), meshes welded along the plane and brushes joined where their union is convex. `args.distance` is the weld distance in units, 0 to 16 and 0.5 by default. The reply has `groups`, `joined`, `welded` and `replaced`. Halves that do not meet, or nothing mirrored selected, is an error and nothing changes |
| `set_editor` `edit_symmetry` | The symmetry of the Vertex and Mesh tools, shared by both, `get_state` reports it as `editor.edit_symmetry`. It is `off`, `x`, `y` or `z` (a plane through the middle of what is edited), or an object `{axis, plane, at}` where `plane` is `center`, `world` or `custom` and `at` is the position on the axis for `custom`, alone it implies `custom`. A value replaces the whole setting, and `max` and `min` are refused since a plane on the side of an object has nothing to mirror |
| `run_action` `move_vertices` | Follows `edit_symmetry` like a drag in the Vertex tool, and the reply has `mirrored`, the number of counterpart vertices that moved with it. If either side would turn a brush concave nothing moves |
| `mesh_edit` `translate` | `symmetry: true` also moves the unselected vertices within half a unit of the mirror image of a moved one, in the same undo step. It uses the plane of `edit_symmetry`, centered on the mesh unless `world` or `custom`, and is an error while that is `off` |
| `get_state` `ui.tool_buttons` | One entry per tool button in the toolbar, in toolbar order, as `{tool, family, rect}`. `family` is `brush` for the brush tools, Texture, Mesh, Volume, Path and Measure, and `terrain` for Sculpt, Blend, Paint and Scatter. Rects are in screen points, multiply by `ui.pixels_per_point` for pixels of a window `screenshot` |
| `get_state` `ui.panels` | The open panel tabs as `{panel, shown}`, where `panel` is the English tab name, such as `Outliner` or `Materials`, and `shown` is true for the tab at the front of its dock. The views are not listed, and a panel that starts closed, like `History` or `Scatter`, appears once it is opened |
| `get_state` `ui.first_run` | `null`, or while the first start card is up, its `rect`, one `{preset, rect, selected}` per keymap preset and the `skip` rect. Click a preset or `skip` with `simulate_input` at the middle of its rect to answer it |
| `get_state` `mirror_planes` | The mirror planes of the selected linked groups, the ones the views draw, as `{axis, at, group, partner, gap}`, `gap` being the distance between the two halves |
| `get_state` `mirror_slides` | The selected linked pairs where one half was slid along the plane, so they have no entry in `mirror_planes`, as `{axis, at, group, partner, slide, distance}`. `slide` is how far `partner` sits from the mirror image of `group`, `distance` its length, what `mirror_again` moves the partner back by. Moving a half across the plane is not a slide, the plane moves with it |
| `validate_map` | Accepts any method or property of the target's Godot class or script as an input. `coplanar_faces` flags faces that will flicker. Every issue has `bounds`. `path` checks a saved map without opening it, `project: true` checks every map in the project |
| `run_action` `place_entities` | Takes `args.classnames`, and `at` and `normal` to rest the entities on a surface the way a drag from the Entities tab does. No classnames is an error |
| `simulate_input` drags | A drag with `"release": false` keeps the button and its modifiers held after the call returns, so a screenshot shows the drag in progress, a card next to the cursor for example. A `release` event in a later call drops it where the pointer is, or at its `x` and `y`, or `world`, and a release with only one of `x` and `y` is an error. The drag and its modifiers stay held, over the real keyboard too, until that release, a `new_map` or a successful `close_tab` action, or MCP going off, so never forget the release. Clicks and drags are refused until then |
| `run_action` `reload_materials` | Rescans the texture folder now, though new files are found on their own anyway |
| `set_editor` `live_link_port` | Talks to a Godot editor whose project sets another `trenchdot/live_link_port` |
| `set_editor` `keymap` | `trenchbroom`, `hammer` or `blender`, `get_state` reports it as `editor.keymap`. Anything else is an error that names the three. The preset changes what keys do in the editor window, `run_action` and the other tools are the same in every preset |
| `run_action` `select_brush_faces` | Swaps the selected brushes and meshes for all their faces, like *Edit > Select > Brush Faces*. Nothing of the kind selected is an error. `select_none` clears objects and faces whatever tool is active |
| `run_action` `object_mode`, `face_mode` | The Blender preset's 1 and 3. `face_mode` turns the selected brushes and meshes into their faces, `object_mode` turns selected faces back into their owners, and both set `editor.select_mode` in `get_state` to `object` or `face`. `select_all_in_mode` then selects every object, or in `face` mode every face of the visible, unlocked brushes and meshes. Only clicks with the Blender preset read the mode |
| `run_action` `select_touching`, `select_inside` | Need brushes selected, faces alone or an empty selection are an error and nothing changes |
| `scatter` `preset` | Adds the embedded Blockbench models a preset needs. A glTF preset in a project without the nature pack is an error that names `project_content` |
| `bake_lighting` | Bakes like [Bake Lighting](editor/light-baking.md) and waits for it. Options left out keep what the map was last baked with. `bake: false` only switches the `view`, `remove: true` drops the bake. `out_of_date` says whether the map changed since |
| `project_content` | `install: "nature"` or `"demo"` downloads from the release and answers once the files are in. `install: "gameplay"` copies the Gameplay entities pack from the project's addon, also inside scripts. Without arguments it says what the project has |
