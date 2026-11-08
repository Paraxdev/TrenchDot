# Tips

Small things that save time once you know them. Each one links to the page with the details.

## In the editor

### Open the manual at what you point at

Point at a tool, a panel tab, a menu entry or an Inspector section and press **F1**, and the matching page of this
manual opens. Over anything else F1 opens the command palette, which you also get with Ctrl+Shift+P. Type the name of
any command there instead of hunting through the menus. See
[Help and the manual](editor/interface.md#help-and-the-manual).

### Pick the object behind another

Click again at the same spot within about a second and a half, or Alt+click in a 2D view, to select the next object
under the cursor, such as a wall under its ceiling in the Top view. The Outliner is the other way to reach something
hidden. See [Mouse in the views](editor/shortcuts.md#mouse-in-the-views).

### Draw matching floors from the Top view

Select a floor and draw in the Top view. Each new brush takes the floor's height and ends up selected, so a row of
floors or walls needs no trip to the other views. See
[Draw and move](editor/brushes.md#draw-and-move).

### Jump between camera spots

Ctrl+Shift+1 to 9 stores the 3D camera as a bookmark in the map, Ctrl+1 to 9 jumps back to it. Shift+Space maximizes
the view under the pointer, press it again to get the others back. See [Camera](editor/interface.md#camera).

### Focus on one part of a big map

Ctrl+J hides everything but the selection, Ctrl+Shift+H shows it all again. For longer work, a
[cordon](editor/organizing.md#cordon) keeps only one area visible and selectable and is saved with the map.

### Keep reference geometry out of the game

Hiding a layer does not stop Godot from building it. Right click the layer and pick *Toggle Omit From Export* instead.
See [Keeping a layer out of Godot](editor/organizing.md#keeping-a-layer-out-of-godot).

### Get back a lost map

Every save keeps the previous version as `<map>.tdm.bak`, and unsaved changes are autosaved every three minutes. See
[Saving and recovery](editor/saving.md).

## In Godot

### Try a change without saving it

*Godot > Build in Godot* sends the map as it is right now to the Godot editor, saved or not. See
[Three ways to build](godot/building.md#three-ways-to-build).

### Hide the outside of a sealed indoor map

The walls of a closed building have outer faces nobody inside can ever see. Press Escape so the Inspector shows
*Map (worldspawn)* and set `_cull_void_faces` to `1`, and the build leaves those faces out. Turn on
*View > Preview void culling* to see the result in the editor first.

If the map has a gap to the outside, it leaks and keeps every face. The Issues panel then names the entity that sees
out, and a line in the views shows the way to the gap. It is off by default since an outdoor map always leaks. See
[Faces that face the void](godot/building.md#faces-that-face-the-void).

### See your saves in a running game

Add the hot reload autoload to your project and start the game from the Godot editor. Each save in TrenchDot then
rebuilds the map in the running game. See [Hot reload in a running game](godot/live.md#hot-reload-in-a-running-game).

### Watch triggers fire

Add a `TrenchDotDebugOverlay` node and press F3 while the game runs. It lists outputs as they fire and draws every
trigger volume. See [F3 in game](gameplay/debugging.md#3-f3-in-game).
