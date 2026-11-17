---
source: e038948c3cf9dabdef8c6212475c2c269d817346
---

# Terrain { #terrain }

Un terrain est une grande grille de hauteurs que vous sculptez comme de l'argile, avec jusqu'à quatre textures peintes
dessus. Servez-vous-en pour le sol à ciel ouvert, comme des collines, des plages et des vallées, là où façonner des brushes
à la main prendrait une éternité.

Cette page explique comment en créer un et le modeler avec l'outil Sculpter (G). Le texturer est expliqué dans
[Calques de terrain](terrain-layers.md) et [Peinture de terrain](terrain-painting.md).

Un terrain peut grandir après sa création, jusqu'à 2048 cellules le long de chaque axe, soit avec le bloc **Taille** de
l'Inspecteur ([Changer la taille](#changing-the-size)), soit en faisant glisser son bord dans une vue 2D avec l'outil
Sélectionner ([Faire glisser un bord](#dragging-an-edge)). Il contient jusqu'à quatre calques, que vous peignez avec
l'[outil Mélange](terrain-painting.md).

Comme le sol est un champ de hauteurs, il ne peut pas se replier sur lui-même, voyez
[Cavernes et surplombs](caves-overhangs.md) pour savoir quoi faire à la place.

## Créer un terrain { #creating-a-terrain }

1. Ouvrir la boîte de dialogue

    Allez dans *Terrain > Créer un terrain…* dans la barre de menus.

    ![Le menu Terrain ouvert avec Créer un terrain… survolé](../assets/terrain/menu-create-terrain.png)

2. Choisir une taille

    **Résolution** et **Taille de cellule** décident de sa taille, et la ligne **Taille** en dessous montre le résultat en
    unités et en mètres. La valeur par défaut de 129 x 129 sommets avec des cellules de 64 unités donne 8192 unités, soit
    256 m. Voir [Choisir la taille de cellule](#picking-the-cell-size) plus bas.

    ![La boîte de dialogue Créer un terrain avec ses valeurs par défaut : 129 x 129, cellules de 64 u, Collines, et les quatre champs de calque](../assets/terrain/create-terrain-dialog.png)

3. (Facultatif) Choisir une forme de départ

    **Forme** est le sol dont vous partez. C'est Collines par défaut, les autres sont Plat, Montagne, Île, Vallée et
    Crêtes. Les lignes dessous règlent le bruit dont il est fait :

    | Ligne | Signification |
    | --- | --- |
    | Hauteur | La hauteur que prend le sol, 512 u par défaut |
    | Taille des reliefs | La largeur d'une colline ou d'une vallée, 2048 u par défaut |
    | Octaves / rugosité | Plus d'octaves et une rugosité plus haute ajoutent des bosses plus fines par-dessus |
    | Érosion | Combien de passes laissent les pentes raides s'affaisser, 20 par défaut |
    | Graine | La même graine redonne le même sol |

    **Importer un PNG…** à côté de **Carte de hauteur** lit à la place une image en niveaux de gris (PNG, TGA ou BMP),
    où les pixels plus clairs sont plus hauts, jusqu'à **Hauteur**. **Effacer** revient à la forme générée. La forme et
    la carte de hauteur ne valent qu'ici, à la création.

4. (Facultatif) Choisir les calques

    **Calque de base**, **Calque de pente**, **Calque de sommet** et **Calque bas** sont les quatre textures du terrain,
    elles commencent comme `dev/green`, `dev/grey`, `dev/blue` et `dev/orange`. **Utiliser l'actuel** met le matériau
    sur lequel vous avez cliqué en dernier, et **tuile** est le nombre d'unités que couvre une répétition. **Peindre les
    calques selon pente et hauteur** est coché par défaut, les calques sont peints tout de suite, voir
    [Peinture auto](terrain-painting.md#auto-paint).

5. Cliquer sur **Créer** en bas

    Le terrain est centré sur le curseur 3D, ou sur le **Centre** que vous saisissez une fois **Curseur 3D** décoché, et
    cette hauteur devient le zéro du terrain. Vous devriez voir le nouveau terrain sélectionné, avec l'outil Sculpter
    déjà actif.

## Choisir la taille de cellule { #picking-the-cell-size }

| Réglage | Signification |
| --- | --- |
| Résolution | Combien de sommets courent le long de chaque côté, l'une des valeurs 33, 65, 129, 257, 513 ou 1025. Plus de sommets gardent plus de détail mais coûtent plus à dessiner et à sculpter |
| Taille de cellule | La distance en unités de carte entre deux sommets voisins, de 4 à 1024 |

257 sommets avec des cellules de 64 unités, par exemple, donnent 16384 unités, soit 512 m.

Rien ne se trouve entre deux sommets, la taille de cellule est donc le plus fin détail que le terrain peut contenir. Une
chaîne de montagnes veut de grandes cellules, un jardin de petites.

Mauvais choix ? Vous pouvez [changer la taille](#changing-the-size) à tout moment après avoir créé le terrain.

**Cellules par chunk** dans l'Inspecteur (32 par défaut) découpe le terrain en morceaux carrés pour le dessin et la
collision. De petits chunks se reconstruisent plus vite pendant le sculptage, de plus grands donnent moins de nœuds dans
Godot.

## Changer la taille { #changing-the-size }

Un terrain n'est pas bloqué à la taille de sa création. Vous pouvez ajouter des cellules de n'importe quel côté, en
retirer ou changer la taille de cellule, et le sol que vous avez déjà sculpté reste exactement là où il était dans la
carte.

1. Sélectionner le terrain

    Cliquez dessus dans une vue ou dans l'Outliner. Son bloc **Taille** se trouve dans l'Inspecteur sous **Origine**.

2. Saisir le changement

    | Champ | Ce que fait la saisie |
    | --- | --- |
    | **Cellules** | Fixe le total le long de X et de Z. Les deux côtés opposés grandissent ou rétrécissent de façon égale, la cellule impaire allant au côté est ou sud |
    | **Nord (-Z)**, **Sud (+Z)**, **Ouest (-X)**, **Est (+X)** | Ne change qu'un côté. Un nombre positif ajoute ce nombre de cellules et un négatif en retire, donc `+8` sur Est en ajoute 8 et `-2` sur Sud en rogne 2 |

    ![Le bloc Taille avec 8 cellules ajoutées à l'est et 2 retirées au sud, et la ligne Après qui montre le résultat](../assets/terrain/inspector-size.png)

    Rien ne change encore. Vos saisies sont un brouillon, dessiné en contour dans les vues, et la ligne **Après** sous
    **Appliquer** vous dit les cellules et la taille que vous obtiendriez.

3. Choisir ce que reçoit le nouveau sol

    **Nouveau sol** apparaît dès qu'un côté grandit. **Hauteur du bord** prolonge le bord le plus proche, la hauteur
    comme la peinture, donc le terrain devient simplement plus long. **Zéro** donne aux nouvelles cellules la hauteur 0
    et le premier calque, pour une bande plate où construire. La première nouvelle cellule descend quand même du vieux
    bord jusqu'à 0, donc la partie plate commence une cellule plus loin.

4. Cliquer sur **Appliquer**

    Le terrain change en une seule étape, et Ctrl+Z le remet exactement comme avant. **Réinitialiser** abandonne le
    brouillon à la place.

Tout ce qui est gardé reste à la même position dans le monde qu'avant : hauteurs, peinture et trous des cellules qui
restent. Si vous ajoutez des cellules du côté ouest ou nord, l'**Origine** du terrain se déplace vers l'extérieur pour
suivre.

**Taille de cellule** marche autrement, elle reconstruit la grille avec le nouvel espacement :

* Le terrain garde sa taille dans la carte, à la cellule entière la plus proche, donc le nombre de cellules change avec
  elle. Les hauteurs et la peinture sont mélangées entre les anciens sommets.
* Quand la taille n'est pas un multiple de la nouvelle cellule, le terrain grandit ou rétrécit d'au plus une
  demi-cellule, et **Les cellules entières changent la taille de** sous **Après** vous dit de combien. Remettre
  l'ancienne taille de cellule n'annule pas cet arrondi, Ctrl+Z si.
* Des cellules plus grandes jettent du détail, et l'Inspecteur vous prévient avec **Les détails fins sont perdus** avant
  d'appliquer. Des cellules plus petites gardent la forme mais ne peuvent pas ajouter un détail qui n'a jamais existé.
* Les côtés s'appliquent d'abord, puis la taille de cellule.

> [!NOTE]
> Un terrain a entre 1 et 2048 cellules le long de chaque axe, et une taille de cellule entre 0.001 et 100000 unités. Un
> changement qui sort de ces limites, ou qui retirerait toutes les cellules, est refusé avec la raison affichée en
> rouge, et **Appliquer** reste grisé.

### Faire glisser un bord { #dragging-an-edge }

Vous pouvez aussi changer la taille directement dans les vues **Dessus**, **Face** et **Côté**, tant qu'un seul terrain
est sélectionné, non verrouillé, sans aucune face sélectionnée. Avec l'outil Sélectionner, passez le pointeur sur l'un
de ses bords jusqu'à ce que le curseur devienne une flèche de redimensionnement et que le bord s'allume, puis faites-le
glisser vers l'extérieur pour ajouter des cellules ou vers l'intérieur pour les rogner. Le bord se cale sur des cellules
entières et la barre d'état montre le nouveau nombre de cellules. Chaque glissement est une étape que Ctrl+Z annule.

![Un terrain dans la vue Dessus dont le bord est a été tiré de quatre cellules vers l'extérieur](../assets/terrain/edge-drag.png)

Un glissement agit comme une saisie dans le champ de ce côté suivie d'un clic immédiat sur **Appliquer**. Le nouveau sol
prolonge le bord, comme le fait **Hauteur du bord**, et un terrain ne descend jamais sous 1 cellule.

Dans les vues Face et Côté, seules les extrémités gauche et droite se saisissent, puisque la hauteur du sol vient du
sculptage. Saisissez plutôt dans le bloc **Taille** quand vous voulez un sol **Zéro** ou une nouvelle taille de cellule.

### Le scatter sur le terrain { #scatter-on-the-terrain }

Le [scatter](scatter.md) peint sur le terrain reste au sol quand vous le redimensionnez :

* Les instances au-dessus d'un sol encore là montent ou descendent avec la nouvelle hauteur, donc rien ne flotte ni ne
  s'enfonce.
* Les instances au-dessus d'un sol rogné sont retirées par défaut. S'il y en a, une case **Retirer n instances scatter
  du sol retiré** apparaît sous **Appliquer**. Décochez-la pour les garder où elles sont, flottant dans le vide, si vous
  comptez les déplacer vous-même.
* Tirer un bord vers l'intérieur les retire toujours, et Ctrl+Z les ramène.
* Un ensemble de scatter verrouillé ou masqué, ou placé dans un calque verrouillé ou masqué, reste tel quel.

## Sculpter { #sculpting }

Appuyez sur G, choisissez un mode dans le premier menu de la barre d'options de l'outil en haut à gauche et faites
glisser sur le terrain. Cela agit sur les terrains sélectionnés, ou sur tous quand rien n'est sélectionné.

![Le menu des modes de l'outil Sculpter ouvert dans la barre d'options de l'outil, à côté des champs de rayon et de force](../assets/terrain/sculpt-modes.png)

Avec Élever, gardez le bouton de la souris enfoncé et le sol continue de monter sous le brush.

![Maintenir Élever avec un brush de 900 unités fait sortir une colline de l'île](../assets/terrain/sculpt-raise.gif)

| Mode | Ce qu'il fait |
| --- | --- |
| Élever, Abaisser | Monte la hauteur ou la creuse tant que vous maintenez le bouton. Un clic fait une petite bosse |
| Lisser | Détend chaque sommet vers ses voisins, pour adoucir les bosses et les crêtes vives |
| Aplanir | Tire le sol à la hauteur où le trait a commencé, pour construire des plateformes, des routes et des plateaux |
| Bruit | Ajoute de petites bosses aléatoires, pour qu'un sol plat paraisse moins artificiel |
| Terrasser | Cale les hauteurs sur des paliers de **pas** unités (32 par défaut), pour des rizières ou des falaises en gradins. Le champ apparaît à côté du mode |
| Peindre l'alpha, Effacer l'alpha, Peindre le calque | Peignent au lieu de modeler. Les modes alpha sont pensés pour les displacements, Peindre le calque peint le calque de terrain que vous réglez dans le champ **calque** à côté du mode. Sur un terrain, l'[outil Mélange](terrain-painting.md) le fait avec plus de contrôle |
| Trou, Reboucher | Découpe des cellules entières ou les referme, collision comprise. Servez-vous-en pour des entrées de grotte ou de bunker, voir [Cavernes et surplombs](caves-overhangs.md) |

Ctrl+molette redimensionne le brush, Shift échange élever et abaisser, et maintenir Ctrl lisse quel que soit le mode.

Le champ **force** de la barre d'options règle la vitesse à laquelle le brush travaille. Élever et Abaisser déplacent le
centre du brush de 8 unités par seconde par point de force, quel que soit le **rayon**, si bien que la valeur par défaut
4 monte de 32 unités par seconde. Pour des collines et des îles, montez la force plutôt que de maintenir une minute.

Gardez le rayon au-dessus de la taille de cellule du terrain, un brush plus petit ne pousse que le seul sommet qu'il a
dessous (la barre d'état vous prévient quand cela arrive).

Pour recommencer, *Terrain > Sculpter > Aplanir*, ou **Aplanir** dans l'Inspecteur, remet tout le terrain sélectionné à
sa hauteur zéro.

> [!TIP]
> Modelez avec un grand brush et lissez après chaque passe grossière. Les petits brushes laissent des bosses.

## Symétrie { #symmetry }

Pour sculpter les deux côtés d'une vallée ou d'une île en même temps, activez la symétrie. Les outils **Sculpter** et
**Mélange** ont tous deux le contrôle, il se trouve dans la barre d'options de l'outil, juste après **force** et
les champs du mode.

![Le menu de symétrie ouvert dans la barre d'options de l'outil Sculpter, avec désactivée, X, Z et X et Z](../assets/terrain/symmetry-menu.png)

| Contrôle | Ce qu'il fait |
| --- | --- |
| Menu **symétrie désactivée** | Choisissez **X** pour refléter les moitiés ouest et est l'une dans l'autre, **Z** pour le nord et le sud, ou **X et Z** pour les deux. Le menu fermé affiche alors **symétrie X**, **symétrie Z** ou **symétrie X et Z** |
| Menu **centre du terrain** | N'apparaît que lorsque la symétrie est activée. Il décide où se trouvent les plans de miroir : **centre du terrain** (le milieu de chaque terrain), **origine du monde** ou **personnalisé** |
| Champs **x** et **z** | Seulement avec **personnalisé**, un par axe reflété, la position du plan en unités. Ils partent des plans que vous utilisiez à l'instant, donc rien ne saute |

Quand la symétrie est activée, les vues dessinent les plans tant que Sculpter ou Mélange est l'outil actif, en contour
rouge pour X et bleu pour Z. Une ligne suit le sol là où chaque plan le coupe, et une copie plus pâle de l'anneau du
brush montre où les touches reflétées tomberont avant que vous n'appuyiez.

![La vue 3D avec la symétrie X et Z, montrant les deux plans de miroir et l'anneau du brush répété dans les quatre quarts](../assets/terrain/symmetry-planes.png)

Faites glisser comme d'habitude. Chaque touche est appliquée sous le curseur et à ses endroits reflétés, et le trait
entier reste une seule étape d'annulation. Une touche sur un plan, ou qui chevauche son propre reflet, compte une fois,
si bien que le sol ne monte pas deux fois plus haut à cet endroit. Le bruit, le bruit de mélange et le spray donnent une
vraie image miroir au lieu d'un nouveau motif aléatoire.

Avec **centre du terrain**, chaque terrain se reflète autour de son propre milieu, et les displacements se reflètent
autour du milieu de tous ensemble. Sculpter et Mélange partagent le réglage. Chaque onglet de carte garde le sien, une
carte nouvelle ou ouverte reprend celui que vous utilisiez mais repasse de **personnalisé** à **centre du terrain**,
et il repart désactivé à chaque lancement de l'éditeur.

> [!TIP]
> Pour refléter à travers plusieurs terrains côte à côte, ou autour d'un point qui n'est pas leur milieu, utilisez
> **origine du monde** ou **personnalisé**. Placez le plan sur une ligne de sommets ou juste à mi-chemin entre deux pour
> une correspondance exacte, toute autre position ne reflète qu'approximativement.

<div class="td-mcp" markdown>

L'action `sculpt`, l'outil `blend` et `terrain_edit` (`sculpt`, `sculpt_path`, `paint_path` et `holes` avec `center`)
acceptent un argument `symmetry`, `off`, `x`, `z`, `xz` ou un objet `{axes, center, x, z}`. `set_editor` règle celui
qu'utilise la barre d'outils et `get_state` le signale, voir [MCP](../mcp.md).

</div>

## Dans Godot { #in-godot }

Chaque terrain devient un nœud `TrenchDotTerrain`, un `StaticBody3D` (le nœud Godot pour un sol solide qui ne bouge
jamais) dont la collision correspond à ce qu'il dessine. Pour le code de gameplay, `height_at(x, z)` renvoie la hauteur
du sol à une position locale, pratique pour poser des choses sur le sol à l'exécution.

<div class="td-mcp" markdown>

## MCP { #mcp }

L'opération `sculpt` de l'outil `terrain_edit` applique un seul coup de pinceau, et sa force est la distance dont ce coup
déplace le centre, là encore quel que soit le rayon. Maintenir un trait pendant une seconde à la force 4 équivaut à un
coup de force 32.

Le même changement de taille est `terrain_edit` avec `op: "resize"`, qui prend `north`, `south`, `west` et `east` en
cellules, ou `cells: [x, z]`, et `cell_size`. Ajoutez `dry_run: true` pour lire d'abord les cellules, les limites,
`scatter_removed` et `detail_lost` du résultat. `fill` vaut `edge` ou `zero` et `scatter` vaut `drop` ou `keep`, comme
dans l'Inspecteur. Les limites sont dans [MCP](../mcp.md).

</div>
