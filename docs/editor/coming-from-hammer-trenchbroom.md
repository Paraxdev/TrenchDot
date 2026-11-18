# Coming from Hammer or TrenchBroom

TrenchDot is a brush editor in the TrenchBroom mould, and it builds Godot scenes instead of compiling for an engine.
You draw solid [brushes](brushes.md) on a grid, texture them, add entities and play, so most of what you know carries
over. This page maps the names, keys and mouse gestures, and lists the places where TrenchDot does something else.
[Build your first room](../getting-started.md) walks you through a whole room if you would rather start there.

## Pick your keys

The first time the editor starts, the card in the middle of the 3D view asks which keys you want. Click the row of the
editor you know, TrenchBroom is already marked, or **Skip** to keep it. That is the only question the editor asks.

![The first start card in the 3D view, with the four steps of the loop and a row each for the TrenchBroom, Hammer and Blender keys, TrenchBroom marked, and Skip at the bottom right](../assets/coming-from-hammer-trenchbroom/keymap-choice.png)

You can change your mind at any time with *File > Preferences > Keymap preset*, or with **Preset** at the top of
*Help > Keyboard Shortcuts…*. Keys you set yourself there stay on top of any preset.

The Hammer preset moves about a dozen actions to the keys Hammer uses, and an action that loses its key to one of
them gets a new one, Close Tab moves to Ctrl+F4 so that Ctrl+W can be Move to World. The mouse works as it does for
TrenchBroom whatever you pick.

## Where things are

| Hammer | TrenchBroom | TrenchDot |
| --- | --- | --- |
| Block tool, a solid | Drag in a view | Drag in any view with the Select tool (Q) to draw a box [brush](brushes.md#draw-and-move), Ctrl+Shift+B for arches, stairs and cylinders |
| Selection tool | Select | **Select** tool, Q |
| worldspawn | worldspawn | The map's worldspawn holds the world brushes, select nothing to see its keys in the Inspector |
| Tie to Entity | *Create Brush Entity* in the right click menu | Select the brushes, right click and pick a class under *Create Brush Entity*, or double click a brush entity card in the **Entities** panel, see [Brush entities](../gameplay/placing.md#brush-entities) |
| Move to World | *Move brushes to Entity* in the right click menu | Ctrl+Shift+W, or Ctrl+W in the Hammer preset |
| Entity tool | Drag from the entity browser | Drag a card from the **Entities** panel into a view, see [Placing entities](../gameplay/placing.md#placing-entities) |
| VisGroups | Layers | [Layers](organizing.md#layers) in the **Outliner**, the eye hides one and the lock stops edits, there are no automatic VisGroups |
| Groups | Groups | [Groups](organizing.md#groups), Ctrl+G |
| Carve | CSG subtract | *Brush > CSG > Subtract*, Ctrl+K, see [CSG](brushes.md#csg) |
| Make Hollow | CSG hollow | *Brush > CSG > Hollow*, Ctrl+Shift+K |
| Clipping tool | Clip tool | **Clip** tool, C |
| Vertex Manipulation | Vertex tool | **Vertex** tool, V |
| Texture Application tool, Face Edit Sheet | Face inspector, material browser | **Texture** tool (Shift+T), the **Alignment** section of the Inspector and the **Materials** panel, see [Texturing](texturing.md). The [UV Editor](uv-editor.md) draws faces flat |
| Displacements | n/a | The **Sculpt** tool shapes [displacements](brushes.md#displacements) and terrain |
| FGD | Entity definition files | The entities of your Godot project, which the addon writes to `trenchdot_game.json` and the **Entities** panel lists |
| Entity I/O | n/a | **Outputs** at the bottom of the Inspector, see [How entity I/O works](../gameplay/io.md) |
| Prefabs | n/a | [Prefabs](organizing.md#prefabs) and the [prefab library](prefab-library.md) |
| Cordon | n/a | [*View > Cordon*](organizing.md#cordon) |
| VMF or MAP file | MAP file | A `.tdm` map. [Import](importing.md) a `.vmf` or `.map` to bring a level over |
| Compile, Run Map | Compile, Launch | There is no compile step. Save the map and Godot rebuilds it, see [Rebuild on save](../godot/live.md#rebuild-on-save). F5 runs the Godot project |

Map units are 32 to the meter and the default grid step is 16.

## Keys

TrenchBroom keys are the defaults of TrenchBroom 2026.2. An empty cell means that program has no key for it.

| Action | TrenchBroom | TrenchDot | Hammer preset |
| --- | --- | --- | --- |
| Select tool | Shift+Esc leaves the current tool | Q | Shift+S |
| Clip tool | C | C | Shift+X |
| Vertex tool | V | V | Shift+V |
| Texture tool | | Shift+T | Shift+A |
| Rotate tool | R | R | R |
| Scale tool | T | T | T |
| Shear tool | G | Shift+R | Shift+R |
| Subtract, hollow | Ctrl+K, Ctrl+Shift+K | Ctrl+K, Ctrl+Shift+K | Ctrl+K, Ctrl+Shift+K |
| Convex merge | Ctrl+J | Ctrl+M | Ctrl+Shift+M |
| Intersect | Ctrl+L | Ctrl+L | Ctrl+Shift+L |
| Duplicate, duplicate linked | Ctrl+D, Ctrl+Shift+D | Ctrl+D, Ctrl+Shift+D | Ctrl+D, Ctrl+Shift+D |
| Group, ungroup | Ctrl+G, Ctrl+Shift+G | Ctrl+G, Ctrl+Shift+G | Ctrl+G, Ctrl+Shift+G |
| Select all | Ctrl+A | Ctrl+A | Ctrl+A |
| Select none | Ctrl+Shift+A | Ctrl+Shift+A, Esc | Esc |
| Invert selection | Ctrl+Alt+A | Ctrl+I | Ctrl+I |
| Select touching | Ctrl+T | Ctrl+T | Ctrl+T |
| Select inside | Ctrl+E | Ctrl+Shift+T | Ctrl+Shift+T |
| Hide selection | Ctrl+Alt+I | Ctrl+H | H |
| Isolate selection | Ctrl+I | Ctrl+J | Ctrl+H |
| Show all | Ctrl+Shift+I | Ctrl+Shift+H | U |
| Halve, double the grid | `-` and `+` | `[` and `]`, or `-` and `=` | `[` and `]`, or `-` and `=` |
| UV lock | U | Ctrl+Shift+U | Ctrl+Shift+U |
| Frame the selection | Ctrl+U | F or Ctrl+U | F or Ctrl+U |
| Maximize the view | Ctrl+Space | Shift+Space | Shift+Space |
| Repeat the last action | Ctrl+R | Ctrl+R | Ctrl+R |

Hammer's own key for Tie to Entity is Ctrl+T, TrenchDot has none, see [below](#where-trenchdot-differs). The full list of
keys is on [Shortcuts](shortcuts.md).

## Mouse

Hammer lets you pick its camera controls in its options, so this table compares TrenchBroom, whose gestures TrenchDot
follows.

| Gesture | TrenchBroom | TrenchDot |
| --- | --- | --- |
| Look around in 3D | Right drag | Right drag |
| Fly in 3D | WASD while the right button is held | WASD while the right button is held, Q and E go down and up, Shift is faster |
| Move forward and back in 3D | Wheel | Wheel |
| Pan in 3D | Middle drag | Middle drag |
| Orbit | Alt+right drag | Alt+left drag |
| Pan in a 2D view | Middle or right drag | Middle or right drag |
| Zoom in a 2D view | Wheel | Wheel |
| Select | Click | Click |
| Add to or remove from the selection | Ctrl+click | Ctrl+click |
| Select a group | Click selects the group, double click opens it | Click selects the object inside it, double click selects the whole group |
| Pick what lies behind | Ctrl+wheel | Click again at the same spot, or Alt+click, in the Top, Front and Side views |
| Select a face | Shift+click | Shift+click |
| Add a face | Ctrl+Shift+click | Ctrl+Shift+click |
| Select every face of a brush | Shift+double click | *Edit > Select > Brush Faces*, Ctrl+Shift+A in the Hammer preset |
| Move on the grid | Drag the selection | Drag the selection |
| Move vertically in 3D | Alt+drag | Alt+drag |
| Lock the move to one axis | Shift while dragging | Shift while dragging |
| Duplicate while dragging | Ctrl+drag | Ctrl+drag |
| Push or pull a face | Shift+drag | Shift+drag a selected face in 3D |
| Extrude a new brush | Ctrl+Shift+drag | Ctrl+Shift+drag a selected face in 3D |
| Change the grid size | Ctrl+Alt+wheel | `[` and `]` |
| Open the context menu | Right click | Right click |

## Where TrenchDot differs

* **Double click selects the whole group.** A plain click picks the object inside a group, the other way round from
  TrenchBroom. Linked copies are the exception, a click on one selects the whole group.
* **Alt+left drag orbits**, where TrenchBroom uses Alt+right drag. In the Select tool a drag that starts on the
  selection or its gizmo moves it vertically instead.
* **Hammer's Ctrl+T, Tie to Entity, has no key.** Right click the selected brushes and pick *Create Brush Entity*, or
  double click a card in the **Entities** panel. Ctrl+T selects what the selection touches, as in TrenchBroom.
* **There is no Transform dialog.** Type a position or size into the Inspector, drag the gizmo, use the Rotate tool
  (R) or *Brush > Transform* for flips, mirrors and alignment.
* **Ctrl+Shift+A is Select None**, as in TrenchBroom. The Hammer preset gives it to Select Brush Faces, which picks every face of the selected brushes to texture them one by one, and Esc clears the selection there.
* **The Hammer preset only moves keys.** Mouse gestures stay as above, and the tools Hammer has no key for keep the
  TrenchBroom ones.
* **There is no compile.** Textures, entities and lighting come from your Godot project, see
  [Build your first room](../getting-started.md).

More about the move from other tools is in [Importing Hammer and TrenchBroom maps](importing.md) and
[Shortcuts](shortcuts.md).
