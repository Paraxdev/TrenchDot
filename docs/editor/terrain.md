# Terrain

A terrain is a big grid of heights you sculpt like clay, with up to four textures painted over it. Use it for open
ground like hills, beaches and valleys, where shaping brushes by hand would take forever.

This page covers making one and shaping it with the Sculpt tool (G). Texturing it is on
[Terrain layers](terrain-layers.md) and [Terrain painting](terrain-painting.md).

## Creating a terrain

1. Open the dialog

    Go to *Terrain > Create Terrain…* in the menu bar.

    ![The Terrain menu open with Create Terrain… hovered](../assets/terrain/menu-create-terrain.png)

2. Pick a size

    **Resolution** and **Cell size** decide how big it gets, and the **Size** row under them shows the result in units
    and meters. The default of 129 x 129 vertices with 64 unit cells gives 8192 units, 256 m. See
    [Picking the cell size](#picking-the-cell-size) below.

    ![The Create Terrain dialog with its defaults: 129 x 129, 64 u cells, Hills, and the four layer fields](../assets/terrain/create-terrain-dialog.png)

3. (Optionally) Pick a starting shape

    **Shape** is the ground you start from. It's Hills by default, the others are Flat, Mountain, Island, Valley and
    Ridges. **Import PNG…** reads a grayscale heightmap instead, where brighter pixels are higher. Both only work here,
    on creation.

4. (Optionally) Pick the layers

    **Base layer**, **Slope layer**, **Peak layer** and **Low layer** are the terrain's four textures, and **Use
    Current** puts in the material you last clicked. With **Paint layers from slope and height** ticked they get painted
    right away, see [Auto Paint](terrain-painting.md#auto-paint).

5. Click **Create** at the bottom

    The terrain is centered on the 3D cursor, or on the **Center** you type in with **3D cursor** unticked, and that
    height becomes the terrain's zero. You should see the new terrain selected, with the Sculpt tool already active.

## Picking the cell size

| Setting | Meaning |
| --- | --- |
| Resolution | How many vertices run along each side, from 33 to 1025. More vertices hold more detail but cost more to draw and sculpt |
| Cell size | The distance in map units between two neighboring vertices |

257 vertices with 64 unit cells, for example, give 16384 units, 512 m.

Nothing sits between two vertices, so the cell size is the finest detail the terrain can hold. A mountain range wants
big cells, a garden small ones.

Picked wrong? You can [change the size](#changing-the-size) any time after creating the terrain.

**Chunk cells** (32 by default) splits the terrain into square pieces for drawing and collision. Smaller chunks rebuild
faster while sculpting, larger ones mean fewer nodes in Godot.

## Changing the size

A terrain isn't stuck at the size you made it. You can add cells on any side, crop them off, or change the cell size,
and the ground you already sculpted stays exactly where it was in the map.

1. Select the terrain

    Click it in a view or in the Outliner. Its **Size** block sits in the Inspector under **Origin**.

2. Type the change

    **Cells** shows how many cells run along X and Z. Type a total there and both opposite sides grow or shrink evenly,
    with the odd cell going to the east or south side. The **North (-Z)**, **South (+Z)**, **West (-X)** and **East
    (+X)** fields change one side only. A positive number adds that many cells and a negative one removes them, so
    `+8` on East adds 8 cells and `-2` on South crops 2.

    ![The Size block with 8 cells added east and 2 removed south, and the After line showing the result](../assets/terrain/inspector-size.png)

    Nothing changes yet. Your edits are a draft, drawn as an outline in the views, and the **After** line under
    **Apply** tells you the cells and size you'd get.

3. Pick what new ground gets

    **New ground** shows up once a side grows. **Edge height** continues the nearest edge, both the height and the
    paint, so the terrain simply gets longer. **Zero** gives the new cells height 0 and the first layer, for a flat
    apron to build on. The first new cell still slopes from the old edge down to 0, so the flat part starts one cell
    out.

4. Click **Apply**

    The terrain changes as one step, and Ctrl+Z puts it back exactly. **Reset** drops the draft instead.

Everything that was kept shows at the same world position as before: heights, paint and holes of the cells that
stay. If you add cells on the west or north side, the terrain's **Origin** moves out to match.

**Cell size** works differently. It keeps the terrain's size in the map to the nearest whole cell and rebuilds the grid
with the new spacing, so the cell count changes with it, and heights and paint are blended between the old vertices.
When the size isn't a multiple of the new cell, the terrain grows or shrinks by up to half a cell, and **Whole cells
change the size by** under **After** tells you how much. Changing the cell size back doesn't undo that rounding,
Ctrl+Z does. Making the cells bigger throws detail away, and the Inspector warns you with **Fine detail is lost** before
you apply. Making them smaller keeps the shape but can't add detail that was never there. The sides apply first, then
the cell size.

> [!NOTE]
> A terrain has between 1 and 2048 cells along each axis, and a cell size between 0.001 and 100000 units. A change
> that goes outside that, or that would crop away every cell, is refused with the reason shown in red, and **Apply**
> stays grayed out.

### Scatter on the terrain

[Scatter](scatter.md) painted on the terrain stays on the ground. Instances over ground that is still there ride up or
down with the new height, so nothing floats or sinks. Instances over cropped ground are removed by default, and when
there are any, a **Remove n scatter instances on removed ground** checkbox appears under **Apply**. Untick it to keep
them where they are, floating over nothing, if you plan to move them yourself. A scatter set that is locked or hidden,
or sits in a locked or hidden layer, is left as it is.

## Sculpting

Press G, pick a mode in the tool options bar at the top left, and drag over the terrain. It works on the selected
terrains, or on all of them when nothing is selected.

![The Sculpt tool's mode menu open in the tool options bar, next to the radius and strength fields](../assets/terrain/sculpt-modes.png)

Keep the mouse button down and the ground keeps rising under the brush.

![Holding Raise with a 900 unit brush builds a hill out of the island](../assets/terrain/sculpt-raise.gif)

| Mode | What it does |
| --- | --- |
| Raise, Lower | Builds height up or digs it down for as long as you hold the button. A click makes a small bump |
| Smooth | Relaxes each vertex towards its neighbors, to soften lumps and sharp ridges |
| Flatten | Pulls the ground to the height where the stroke started, for building pads, roads and plateaus |
| Noise | Adds small random bumps, so flat ground looks less artificial |
| Terrace | Snaps heights to steps of **step** units, for rice fields or stepped cliffs. The field shows up next to the mode |
| Paint Alpha, Erase Alpha, Paint Layer | Paint instead of shaping. The alpha modes are meant for displacements, Paint Layer paints the terrain layer set next to the mode. On a terrain the [Blend tool](terrain-painting.md) does this with more control |
| Hole, Unhole | Cuts whole cells out or fills them back, collision included. Use it for cave or bunker entrances |

Ctrl+wheel resizes the brush, Shift swaps raise and lower, and holding Ctrl smooths whatever the mode.

**Strength** sets how fast the brush works. Raise and Lower move the center of the brush 8 units a second per point of
strength, whatever the radius, so the default 4 builds 32 units a second. For hills and islands turn the strength up
rather than holding for a minute.

Keep the radius above the terrain's cell size, a smaller brush only nudges the one vertex under it (the status bar
tells you when that happens).

To start over, *Terrain > Sculpt > Flatten*, or **Flatten** in the Inspector, sets the whole selected terrain back to
its zero height.

> [!TIP]
> Shape with a large brush and smooth after every rough pass. Small brushes leave lumps.

## In Godot

Every terrain becomes a `TrenchDotTerrain` node, a `StaticBody3D` (Godot's node for solid ground that never moves)
whose collision matches what it draws. For gameplay code, `height_at(x, z)` returns the ground height at a local
position, handy for placing things on the ground at runtime.

<div class="td-mcp" markdown>

## MCP

The `terrain_edit` tool's `sculpt` op applies a single dab, and its strength is how far that dab moves the center, again
whatever the radius. Holding a stroke for one second at strength 4 matches a dab of strength 32.

The same size change is `terrain_edit` with `op: "resize"`, taking `north`, `south`, `west` and `east` in cells, or
`cells: [x, z]`, and `cell_size`. Add `dry_run: true` to read the cells, bounds, `scatter_removed` and `detail_lost` of
the result first. `fill` is `edge` or `zero` and `scatter` is `drop` or `keep`, as in the Inspector. See
[MCP](../mcp.md) for the limits.

</div>
