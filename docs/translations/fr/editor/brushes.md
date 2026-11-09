---
source: b69e3e68d14e4acf224ebc411ee99d88523920f4
---

# Brushes et CSG { #brushes-and-csg }

Les brushes sont ce avec quoi vous construisez la plus grande partie d'un niveau. Ils se dessinent vite, s'aimantent
proprement à la grille et deviennent dans Godot des maillages avec la collision qui va avec. Tout ce qui suit utilise
l'outil **Sélectionner** (Q), sauf si une section nomme un autre outil.

## Dessiner et déplacer { #draw-and-move }

Faites glisser n'importe où hors de la sélection, dans n'importe quelle vue, et une boîte suit la souris. Les vues 2D
affichent sa taille à côté. Dans la vue 3D, elle commence sur la surface sous le curseur et a l'épaisseur d'un pas de
grille. Dans une vue 2D, elle prend la profondeur de ce qui est sélectionné, comme dans Hammer et TrenchBroom. Avec un
sol sélectionné, vous pouvez dessiner d'autres sols de même hauteur depuis la seule vue de dessus. Sans sélection, elle
prend la profondeur du dernier brush que vous avez dessiné, 128 unités pour le tout premier. Décochez *Profondeur des
nouvelles brushes selon la sélection* dans *Fichier > Préférences* pour toujours reprendre la profondeur du dernier
brush.

![La case Profondeur des nouvelles brushes selon la sélection cochée dans les Préférences, sous Préréglage des raccourcis](../assets/brushes/preferences-depth-from-selection.png)

![Les quatre vues pendant qu'une boîte est dessinée en 3D, la nouvelle boîte fait 144 sur 144 unités et un pas de grille, 16 unités, d'épaisseur](../assets/brushes/draw-box-3d.png)

Une fois un brush sélectionné, vous pouvez le déplacer :

| Action | Résultat |
| --- | --- |
| Faire glisser la sélection | La déplace, Shift verrouille sur l'axe principal |
| Ctrl+glisser la sélection | Déplace une copie, l'original reste |
| Alt+glisser la sélection (3D) | Déplace à la verticale. En partant hors de la sélection, Alt+glisser fait orbiter la caméra |
| Flèches, Page Up, Page Down | Décalent d'un pas de grille |
| `[` et `]` | Divisent par deux et doublent la grille |

En 3D, la sélection reçoit aussi un gizmo de déplacement, de rotation et d'échelle. La rotation s'aimante par pas de 15°,
ou de 1° avec Shift. S'il vous gêne, désactivez-le sous *Affichage > Gizmo de transformation*.

## Position et taille exactes { #exact-position-and-size }

Parfois il vous faut des nombres exacts, comme un cutter de porte exactement aussi profond que le mur. Sélectionnez le
brush et saisissez-les dans l'**Inspecteur** à droite.

![L'Inspecteur avec un brush sélectionné, qui montre ses champs de position et de taille](../assets/brushes/inspector-position-size.png)

La position est le coin le plus bas, les plus petits X, Y et Z, c'est-à-dire le coin qui se trouve en général sur la
grille. Une nouvelle taille fait grandir la sélection en s'éloignant de ce coin, la position ne bouge donc pas. Faire
glisser un champ avance par pas de grille tant que l'aimantation est active, une valeur saisie est gardée exactement. Les
mêmes champs apparaissent pour les maillages, une entité brush et une sélection de plusieurs objets.

## Remodeler { #reshape }

Vous pouvez remodeler un brush sélectionné sans quitter l'outil Sélectionner :

| Action | Résultat |
| --- | --- |
| Shift+glisser une face sélectionnée (3D) | Pousse ou tire cette face |
| Ctrl+Shift+glisser une face sélectionnée (3D) | Extrude un nouveau brush à partir d'elle |
| Faire glisser une arête de la sélection (2D) | Redimensionne. L'arête s'allume en jaune et le curseur change là où un glisser redimensionne |

Tant que l'aimantation est active, une face que vous faites glisser se pose sur une ligne de la grille, même si le brush
était hors grille avant. Les faces inclinées se déplacent plutôt par pas de grille.

![Shift+glisser la face latérale d'une boîte sélectionnée dans la vue 3D, la boîte s'étire par pas de grille](../assets/brushes/push-face.gif)

Pour des pentes, des rampes et des murs en biais, appuyez sur C pour l'outil **Clip** :

1. Cliquez deux ou trois points le long de la coupe. Ils s'aimantent à la grille, et en 3D ils se posent sur la surface
   sur laquelle vous cliquez.
2. Appuyez sur Tab pour choisir ce qu'il faut garder, l'avant, l'arrière ou les deux moitiés. Le texte en bas de la vue
   indique laquelle est active.
3. Appuyez sur Enter pour couper. Backspace retire le dernier point si vous avez mal cliqué.

![L'outil Clip avec deux points placés dans la vue de face en travers du coin supérieur d'une boîte, la ligne de coupe apparaît aussi dans la vue 3D](../assets/brushes/clip-points.png)

Pour faire glisser des coins isolés, appuyez sur V pour l'outil **Sommet**, ou sur Tab avec des brushes sélectionnés. Il
affiche des poignées sur les coins, et de petits points bleus au milieu des arêtes et au centre des faces. Cliquez sur un
point pour le sélectionner, ou éloignez-le du brush en le faisant glisser pour y ajouter un coin. Delete retire les
sommets sélectionnés.

![L'outil Sommet sur une boîte coupée, des poignées jaunes aux coins et des points bleus sur les arêtes et les faces](../assets/brushes/vertex-handles.png)

> [!NOTE]
> Un brush ne peut pas avoir de creux, un déplacement qui en créerait un est donc refusé. Vous obtenez un fantôme rouge
> là où il atterrirait et une ligne dans la barre d'état. Tirez plutôt le point vers l'extérieur, ou convertissez le brush
> en [maillage](meshes.md) pour des creux et des surplombs.

## Tourner, mettre à l'échelle et mesurer { #rotate-scale-and-measure }

| Outil | Action | Résultat |
| --- | --- | --- |
| **Rotation** (R) | Faire glisser un anneau autour de la sélection | Tourne par pas de 15°, avec Shift par pas de 1°. Une vue 2D a un seul anneau, autour de l'axe selon lequel elle regarde |
| **Échelle** (T) | Faire glisser une poignée sur les limites de la sélection | Redimensionne sur la grille, avec Alt des deux côtés à la fois pour que le centre ne bouge pas. La vue 3D a une poignée sur chaque côté |
| **Mesurer** (M) | Faire glisser, ou cliquer deux points | Affiche la distance en unités et en mètres, et en bas de la vue ses parties X, Y, Z et horizontale |

![L'outil Mesurer dans la vue 3D, une ligne marquée 230.76 u (7.21 m) et les parties X, Y et Z en bas](../assets/brushes/measure.png)

## Générateur de formes { #shape-generator }

Pour des cylindres, des arches, des escaliers, des toits et d'autres formes qui demanderaient beaucoup de brushes dessinés
à la main, utilisez le Générateur de formes.

1. Dessinez une boîte là où la forme doit aller, et laissez-la sélectionnée.
2. Ouvrez *Brush > Générateur de formes…* (Ctrl+Shift+B).
3. Choisissez la **Forme** et ses options. La boîte de dialogue indique les limites qu'elle remplit, et qu'elle remplace la
   sélection.
4. Cliquez sur **Créer**.

![La boîte de dialogue du Générateur de formes réglée sur Cylindre avec 16 côtés et l'axe Y, qui remplit une sélection de 128 x 192 x 128](../assets/brushes/shape-generator.png)

![La boîte sélectionnée à gauche, le cylindre à 16 côtés qui l'a remplacée à droite](../assets/brushes/shape-generator-box-to-cylinder.png)

Les cylindres et les cônes sont debout par défaut. Choisissez **X** ou **Z** sous **Axe** pour en coucher un dans la même
boîte, pour des tuyaux, des troncs ou une poutre, au lieu de le tourner ensuite. Le tore, le mur en arche, le toit à
pignon, la flèche et la grille sont toujours créés comme [maillage](meshes.md). Les cylindres, les cônes et les sphères
peuvent l'être aussi avec **Créer comme maillage modifiable**, puis **Ombrage lisse** les arrondit. La forme **Texte**
construit des lettres en blocs à partir de brushes, pour des enseignes et des inscriptions au sol.

**Rocher** remplit la boîte d'une pierre. **Rugosité** règle son aspect bosselé, **Facettes** y taille des faces planes
et **Base plate** coupe son dessous à plat pour qu'il repose sur le sol. Changez la **Graine** pour une autre pierre du
même genre. En brush, le rocher est une seule pierre convexe low poly qui fonctionne avec le CSG et entre en collision
comme n'importe quel brush. Avec **Créer comme maillage modifiable**, il garde ses creux et ses surplombs, et **Détail**
règle la finesse de la subdivision de la surface.

![Le Générateur de formes réglé sur Rocher en maillage modifiable, avec Rugosité 0.5, Facettes 5, Base plate 0.3, Détail 3 et Graine 1](../assets/brushes/shape-generator-rock.png)

![Cinq rochers orange sur un sol gris, trois maillages bosselés devant et deux rochers brush à facettes derrière](../assets/brushes/shape-generator-rocks.png)

## Balayer des faces { #sweep-faces }

Pour prolonger un couloir, un tuyau ou une moulure autour d'un angle, balayez les faces de son extrémité. Chaque face
est poussée le long d'un chemin et laisse derrière elle une suite de brushes, avec les matériaux du brush dont elle
vient.

1. Faites Shift+clic sur une face à l'extrémité du tronçon dans la vue 3D, puis Ctrl+Shift+clic sur les autres, par
   exemple le sol et les deux murs d'un couloir.
2. Ouvrez *Brush > Balayer des faces…*. Un contour cyan montre les brushes qu'il créerait.
3. Choisissez **Droit**, **Arc** ou **Courbe en S** et réglez le chemin. **Rayon** se mesure du centre du virage au
   milieu des faces, et **Direction** se voit en regardant les faces.
4. Cliquez sur **Créer**. Les nouveaux brushes sont sélectionnés et rejoignent le groupe ou l'entité du brush dont ils
   viennent, si bien que balayer l'extrémité d'une `func_door` allonge la porte.

![La boîte de dialogue Balayer des faces réglée sur Arc avec Rayon 128, Angle 90, Direction Droite et 8 segments, qui crée 32 brushes](../assets/brushes/sweep-faces-dialog.png)

![Un couloir au sol sombre et à deux murs orange, balayé en ligne droite, autour d'un arc de 90 degrés, puis de nouveau en ligne droite](../assets/brushes/sweep-faces-corridor.png)

Davantage de **Segments** donnent une courbe plus lisse. **Montée** soulève l'extrémité, si bien qu'un arc avec de la
montée devient une rampe en spirale, **Torsion** fait tourner les faces sur elles-mêmes et **Échelle finale** les
agrandit ou les rétrécit en chemin. Un arc plus serré que la largeur des faces se replierait sur son côté intérieur, la
boîte de dialogue indique alors le plus petit rayon qui convient. Toutes les faces choisies avancent dans le même sens,
selon leur direction moyenne, choisissez donc des faces orientées de la même façon, comme l'extrémité d'un couloir. Des
faces inclinées les unes par rapport aux autres partiraient de travers.

<div class="td-mcp" markdown>

L'outil MCP `sweep_faces` prend les mêmes réglages, avec les faces sous forme de paires `[brush id, face index]` ou les
faces sélectionnées quand aucune n'est donnée.

</div>

## CSG { #csg }

Les opérations CSG sont sous *Brush > CSG*, qui contient aussi leurs deux réglages.

![Le menu Brush avec le sous-menu CSG ouvert, qui liste Soustraire, Fusion convexe, Intersection et Évider avec leurs raccourcis, la case Les faces creusées gardent le matériau de la cible et le champ Épaisseur des murs](../assets/brushes/menu-brush-csg.png)

Les quatre ont aussi un bouton dans la barre d'outils, à droite des boutons d'aimantation et de verrouillage UV.

![Les quatre boutons CSG dans la barre d'outils, Fusion convexe survolé avec son infobulle](../assets/brushes/toolbar-csg.png)

| Opération | Touche | Résultat |
| --- | --- | --- |
| Soustraire | Ctrl+K | Creuse la sélection dans tous les brushes qu'elle touche, puis supprime la sélection. Une sélection qui ne touche aucun autre brush est gardée et la barre d'état le dit |
| Évider | Ctrl+Shift+K | Transforme un brush en murs autour de son ancien volume, de 16 unités d'épaisseur sauf si vous changez *Épaisseur des murs* dans le même menu. Un brush trop fin pour deux murs est laissé tel quel |
| Fusion convexe | Ctrl+M | Fond la sélection en un seul brush convexe qui l'enveloppe |
| Intersection | Ctrl+L | Ne garde que là où les brushes sélectionnés se chevauchent |

Pour percer une porte ou une fenêtre, placez un brush là où le trou doit aller et soustrayez-le,
[Premiers pas](../getting-started.md) vous en guide un. Les faces qu'une coupe ouvre prennent le matériau du cutter, sauf
si vous cochez *Les faces creusées gardent le matériau de la cible* dans le même menu.

## Displacements { #displacements }

Pour un sol irrégulier dans un niveau en brushes, comme une pente de gravats, vous pouvez transformer une face en
displacement et la sculpter.

1. Faites Shift+clic sur une face à quatre côtés, ou sélectionnez le brush pour utiliser sa face supérieure.
2. Ouvrez *Brush > Displacement* et choisissez l'une des entrées **Créer**. Une puissance plus élevée donne une grille plus
   fine.
3. L'outil **Sculpter** (G) s'active tout seul, faites glisser sur la face pour la modeler.

![Le menu Brush avec le sous-menu Displacement ouvert, Créer, puissance 2 (5x5), Créer, puissance 3 (9x9) et Créer, puissance 4 (17x17), puis Coudre les displacements et Retirer le displacement](../assets/brushes/menu-brush-displacement.png)

Pour de plus grandes zones en extérieur, un [terrain](terrain.md) convient mieux.
