# Texturing

## Applying materials

Your textures live in the **Materials** panel, at the bottom of the window under the views. The search box at its
top left filters them by name, **Used in map** and **Favorites** narrow the list down, and the slider sets the
thumbnail size.

![The Materials panel with the search box, the folder list, Used in map, Favorites and the size slider, below them the material thumbnails with dev/blue hovered showing its name and size](../assets/texturing/materials-panel.png)

| Do this | Result |
| --- | --- |
| Click a thumbnail | Apply it to the selection, and use it for new brushes |
| Drag a thumbnail onto a face | Apply it to that face |
| Shift while dropping | Apply it to the whole brush or mesh |
| Alt while dropping | Place it as a [decal](decals.md) |

While you drag, a little card next to the cursor reminds you of the modifiers, and the face changes as soon as you let
go.

![Dragging withered/mossy_plaster onto the front of a brick wall, then the same wall with the plaster on its front face only](../assets/texturing/drop-on-face.png)

To texture one face without dragging, Shift+click it in a view first, then click the thumbnail. Ctrl+Shift+click adds
or removes more faces. In the Blender preset, press **3** for Face mode first, then a click picks the face and
Shift+click adds or removes more. While faces are selected the status bar counts them and the brushes they sit on, like "6 faces
on 1 brush".

![The status bar showing 6 faces on 1 brush after Select Brush Faces on a box](../assets/texturing/status-face-chip.png)

Right click a thumbnail for more: add it to your favourites, select the faces that use it, swap it in for the current
material across the whole map, open the hotspot editor, or *Show in File Manager* to open the folder of its texture
with the file selected, for example to edit it.

![The right click menu of a thumbnail with Apply to selection, Add to favourites, Select faces using it, Replace showcase/red_bricks with it in the map, Hotspot Editor, Copy Name and Show in File Manager](../assets/texturing/material-context-menu.png)

Dropping a material on a terrain sets a terrain layer instead, see [Terrain layers](terrain-layers.md).

UV lock (Ctrl+Shift+U) is on by default, so textures stay attached to a brush while you move it.

## Built in colors

The Materials panel always offers `dev/grey`, `dev/dark`, `dev/orange`, `dev/blue` and `dev/green`, grid textures
for blocking out a level before it has art. Color coding rooms with them survives the build, since the addon ships the
same grids and Godot uses them when the project has no texture of that name. A texture of your own at, for example,
`res://textures/dev/grey.png` replaces one.

## Tool textures

The editor draws tool textures see-through with stripes, and the Godot build gives them no visual mesh. Every texture
in the `special/`, `tools/` or `td/` folder counts, and so does any texture named after one of the tools below, such as
`common/caulk`.

![A dev/dark block, a see-through purple special/clip wall in front of it and an orange striped special/trigger box](../assets/texturing/tool-textures.png)

| Texture | In Godot |
| --- | --- |
| `special/clip`, `playerclip`, `nodraw`, `caulk`, `trigger`, `sky` and any other tool | Collides but draws nothing. A trigger keeps its trigger shape |
| `special/skip`, `origin`, `hint`, `hintskip`, `null`, `areaportal` | Builds nothing at all. A brush of only these faces does not collide |
| `special/occluder` | Draws nothing and does not hide neighbors, but adds to the occluder of an entity that builds one, like `func_geo` |

The map settings name the clip, skip, origin and sky textures, `special/clip` and so on by default. An origin brush only
sets its entity's origin with the texture the map settings name.

## The Texture tool

A face's alignment is where its texture sits on it: the offset, scale and rotation. Press **Shift+T** for the Texture
tool, which edits it in place, like Hammer's face edit mode. Click a face to select it. It is tinted red and shows a
blue box around one repeat of the texture, plus two short lines for the U and V directions. The face under the cursor
gets a yellow outline.

![The Texture tool with the front face of a brick wall selected, a blue box marks one texture repeat](../assets/texturing/texture-tool-face.png)

| Input | Result |
| --- | --- |
| Click, Ctrl+click | Click selects a face, Ctrl+click adds or removes one, in every keymap preset |
| Drag a selected face | Slide the texture, the pixel under the cursor stays under it |
| Arrow keys | Slide the selected faces by the grid size in pixels, with Shift by one pixel |
| Ctrl+wheel | Scale the selected faces by 2, with Shift in fine steps |
| Alt+wheel | Rotate the selected faces by 15°, with Shift by 1° |
| Alt+click | Pick up a face's material and alignment |
| Right click | Apply the current material |
| Ctrl+right click | Apply it and reset the alignment |
| Shift+right click | Paste the picked material and alignment |
| Alt+right click | Apply the current material, continuing the selected face's alignment across the edge |

With no face selected, a selected brush counts as all its faces, so you can select a brush, press **Shift+T** and
scroll straight away. Several selected faces or brushes change together.

To work on faces one by one, for example to leave a few out:

1. Select the brushes and choose *Edit > Select > Brush Faces*. In the Hammer preset **Ctrl+Shift+A** does it too, and
    **Ctrl+A** with the Texture tool active selects the faces of the selected brushes

    ![The Edit menu with the Select submenu open, listing All, None, Inverse, Brush Faces and more](../assets/texturing/menu-brush-faces.png)

    The brushes and meshes are swapped for all their faces, tinted red.

2. **Ctrl+click** each face you do not want, to remove it from the selection

The [UV Editor](uv-editor.md) follows the same selection.

Alt+right click is how a texture, like a trim or a row of bricks, runs cleanly around a corner.

The tool options bar at the top has the **Justify** buttons for the selected faces:

| Button | Result |
| --- | --- |
| **Left**, **Right**, **Top**, **Bottom**, **Center** | Push the texture to that side of the face |
| **Fit** | Stretches the texture over the face |
| **Fit W**, **Fit H** | Stretch it only across, or only up |
| **Treat as One** | A tick box. With it ticked the buttons work on all selected faces as one area, so one texture spans all of them |
| **Align to View** | Projects the texture along the 3D camera |

![The Texture tool options bar with Justify Left, Right, Top, Bottom, Center, Fit, Fit W, Fit H, Treat as One, Align to View and Hotspot Fit](../assets/texturing/texture-tool-options.png)

If you prefer numbers, the **Inspector** shows the same face under **Alignment**, with offset, scale and rotation
fields and buttons to flip, turn and reset it. **Texture Tools** below it sets the texel density and copies
alignment from face to face.

![The Inspector with one face selected, the Alignment section with offset, scale and rotation, the Justify buttons and the Texture Tools section](../assets/texturing/inspector-alignment.png)

For faces with explicit UVs, and for meshes, use the [UV Editor](uv-editor.md).

## Texture scale

By default one texture pixel covers one map unit, so a 64 pixel texture repeats every two meters. That suits low
resolution textures, but a 1024 pixel photo would stretch over 32 meters. You can change the size one repeat covers in
two places:

| Where | Use it for |
| --- | --- |
| `metadata/texture_size` on the material, see [Texture size](../godot/materials.md#texture-size) | Every map that uses the material |
| [Texture Settings](light-baking.md#texture-settings) in the Inspector | One map only, or a texture that runs on seamlessly across every face. Its repeat size wins over the material's |

The Inspector line under the material name tells you what a face got, like "1024 x 1024 px, 64 x 64 units a repeat".

## Hotspots

Hotspots let one trim sheet texture lots of differently shaped faces. You mark its rectangles once, then TrenchDot
picks the right one for each face.

1. Right click the trim sheet in the Materials panel and choose **Hotspot Editor**. *Texture > Hotspot Editor…*
    opens it too, for the material of the selected face or else the current one.

    ![The Texture menu with Hotspot Editor… hovered, Hotspot Fit (Alt+H) at the bottom](../assets/texturing/menu-texture.png)

2. Drag on the texture to draw a rectangle, or set a grid and click **Split into grid**. Drag a rectangle to move it,
   its corner handle to resize it, right click deletes it.
3. Click **Save**. The rectangles go into `<texture>.hotspots.json` next to the texture file.
4. Select the faces and use *Texture > Hotspot Fit* (Alt+H), or **Apply to selection** in the editor.

![The Hotspot Editor on withered/concrete_panels split into a 4 by 4 grid, 16 rectangles, unsaved](../assets/texturing/hotspot-editor.png)

Hotspot Fit gives each face the rectangle whose shape matches it best, preferring one that keeps the texture close to
its normal scale.

## Blending a second material

A face shows one material, but you can paint a second one over it, like dirt breaking through grass or moss creeping
up a wall. The face keeps its own material and gets a blend material, and the **Blend** tool paints how much of it
shows. The tool works on brushes, meshes and displacements, it is the same one that
[paints a terrain](terrain-painting.md).

1. Make the second material current

    Press Escape so nothing is selected, then click the material to paint in, for example `showcase/dirt`, in the
    Materials panel. With a selection, that click would retexture it instead.

2. Select the brush and press Shift+G

    The tool options bar at the top left turns into the Blend tool's. To blend only some faces, select them before you
    press Shift+G, the way [Applying materials](#applying-materials) picks single faces. In the Blend tool a click
    paints instead.

3. Click **Set Blend Material**

    It sits to the right of the bar's options, and *Terrain > Blend > Set Blend Material (current)* does the same.

    ![The Blend tool options bar with Set Blend Material, repeat, detile, sharpen and Apply Tiling on the right](../assets/texturing/blend-options.png)

    ![The Terrain menu with the Blend submenu open and Set Blend Material (current) highlighted](../assets/texturing/menu-set-blend-material.png)

    The status bar names the material and how many faces got it. Nothing looks different yet, since no part of
    the second material shows until you paint it in.

    ![The status bar reporting the blend material and the number of faces](../assets/texturing/blend-status.png)

4. Paint

    Drag over the brush in a 3D view. **paint** brings the blend material in, hold Shift to take it back out, and the
    other modes, falloffs, radius and strength work as on [Terrain painting](terrain-painting.md#the-blend-tool). With
    nothing selected, the tool paints every face in the map that has a blend material, so select the brush to keep
    a stroke off the rest.

    ![Dirt painted across the top of a grass slab with the Blend tool](../assets/texturing/blend-result.png)

The blend is stored on the corners of each face, so a plain box face can only fade between its four corners. To paint
a path or a patch, give the face more corners first. Turn it into a [displacement](brushes.md#displacements) with
*Brush > Displacement > Create* (a higher power gives a finer grid), or use a mesh and subdivide the face, see
[Meshes](meshes.md).

The three fields next to **Set Blend Material** change how the blend material looks in Godot, the face's own material
is not touched:

| Field | What it does |
| --- | --- |
| **repeat** | How much more often the blend material repeats across the face, 1 keeps the normal size and 2 doubles the number of repeats |
| **detile** | From 0 to 1, turns and shifts every repeat and blurs the joins, so a path does not show a pattern. 0 leaves the plain repeat |
| **sharpen** | From 0 to 1, how crisp the de-tiled texture stays, 0 is soft and 1 blends only right at the joins. It does nothing while **detile** is 0 |

Changing a field does nothing by itself. Click **Apply Tiling** to give the selected faces, or every face of the
selected brushes, the three values. A face that never got them uses repeat 1 and detile 0.

The fields keep their values between maps, so the next brush starts from the same numbers. **Clear Blend Material** in
*Terrain > Blend* takes the blend material off the selection again.

## Vertex paint

The **Paint** tool (P) paints vertex colors onto brush faces, for dirt, tint and fake lighting. Pick the color in the
tool options bar, next to the brush radius and strength, then drag over the faces. It paints the selected brushes, or
every brush when nothing is selected. Ctrl+wheel resizes the brush.

![The Paint tool options bar with the Color swatch, radius 48 u and strength 4.0](../assets/texturing/paint-tool-options.png)
