# Meshes

Unlike brushes, meshes can have dents, holes and bent faces, so use them for rocks, pipes, smooth arches and other
shapes brushes cannot make.

## Editing a mesh

Select the brushes you want to reshape and choose *Mesh > Edit Mesh*. It converts them to a mesh and starts editing
straight away.

![The Mesh menu with Edit Mesh hovered, then Convert Brushes to Mesh (Ctrl+Shift+E), Convert Meshes to Brushes, Join Meshes (Ctrl+Shift+J) and the Topology, Components, Normals and Shading and Transform submenus](../assets/meshes/menu-edit-mesh.png)

With a mesh selected, **Tab** does the same, and Tab again takes you back to objects. Tab on brushes opens the Vertex
tool instead, so a stray key press never turns a brush into a mesh. The Mesh tool button in the toolbar starts editing
without converting anything. Ctrl+Shift+E only converts, Ctrl+Shift+J joins meshes into one.

While you edit, the tool options bar at the top switches between **Vertex**, **Edge** and **Face**, and has buttons for
the common operations.

![The Mesh tool options bar with Vertex, Edge and Face, then Subdivide, Merge, Fill, Delete, Smooth and Flat and a symmetry menu set to off](../assets/meshes/mesh-tool-options.png)

## Keys

The keys follow Blender. For example, press 3, click the top face of a box and press E, then move the mouse up and
click to extrude it:

![A mesh box in face mode, its top face extruded upwards in grid steps](../assets/meshes/extrude-face.gif)

| Key | Action |
| --- | --- |
| 1, 2, 3 | Work on vertices, edges or faces |
| A, Alt+A | Select all, select none |
| Alt+click, Ctrl+Alt+click | Select the edge loop through the clicked edge, or the ring across it |
| Ctrl+L | Select everything connected to the selection |
| G, R, S | Move, rotate, scale |
| E | Extrude new geometry out of the selection |
| I | Inset, a smaller face inside each selected face |
| Ctrl+B, Ctrl+Shift+B | Bevel edges or vertices, cutting the corner off |
| Ctrl+R | Loop cut, a new ring of edges across the mesh. The wheel or + and - set the number of cuts |
| K | Knife, click two points to cut along that line |
| M | Merge the selected vertices at their center |
| F | Fill, a new face across the selected vertices |
| X or Delete | Delete |
| Shift+D | Duplicate and move |
| P | Separate the selection into a new mesh |
| Alt+F | Flip normals, for faces that show from the wrong side |

While moving, rotating or scaling, press X, Y or Z to lock to that axis, or Shift with the letter to lock it out. Type
a number for an exact amount. Click or Enter confirms, right click or Escape cancels. The hint line at the bottom of the
view lists these as you go.

## Symmetry

For a face, a creature or a car, shape one side and let the other follow. At the right end of the Mesh tool's options
bar, behind a separator, sits a menu that reads **symmetry off**. Pick **X**, **Y** or **Z** and it reads **symmetry X**,
**symmetry Y** or **symmetry Z**. A second menu after the label **Symmetry Plane** then shows where the plane sits, **selection center** (the middle of the
meshes you are editing), **world origin** or **custom**, and a number field next to it takes the position for
**custom**. It works like X mirror in Blender's edit mode.

![The symmetry menu open at the end of the Mesh tool's options bar, listing off, X, Y and Z](../assets/meshes/mesh-symmetry.png)

While symmetry is on, the views draw the plane over the mesh, red for X, green for Y and blue for Z. Move, rotate or
scale vertices, edges or faces, with G, R, S, the gizmo or a drag, and every other vertex of the same mesh that sits
within half a unit of the mirror image of a moved one moves the mirrored way, in the same undo step. A vertex without a
partner moves alone. One you selected yourself always moves exactly as you move it, so selecting both sides, or the
whole mesh, behaves as if symmetry were off.

Only moving follows. Extrude, inset, bevel, loop cut, knife, merge and delete act on the selection and nothing else, so
do them on both sides or build the shape on one side first. *Mesh > Transform > Mirror X*, **Y** and **Z** stay what
they were, a one-off flip of the selected meshes.

The setting is not saved, it is off again each time you start the editor, and the [Vertex tool](brushes.md#symmetric-editing)
shares it.

To get a second half you can keep editing, mirror the mesh as a [linked copy](brushes.md#mirror) and click
**Apply Mirror** when you are happy. The two halves become one mesh with the seam welded, vertices within half a unit of
the plane count as part of it, and the symmetry menu then keeps editing the whole mesh evenly. Let the halves meet at the
plane rather than overlap, since only the seam is welded and an overlapping part would stay doubled.

<div class="td-mcp" markdown>

`mesh_edit` with the op `translate` takes `symmetry: true` to move the counterparts too. It uses the plane that
`set_editor` `edit_symmetry` holds, and an `edit_symmetry` of `off` is an error there.

</div>

## Gizmo

Double click an element to put a move, rotate and scale gizmo on it. It is the clean way to slide a vertex along one
axis, and it works in the 2D views too. Escape hides it and keeps the selection.

The *Mesh* menu holds the rest, such as subdivide, triangulate, solidify, mirror and smooth shading.
