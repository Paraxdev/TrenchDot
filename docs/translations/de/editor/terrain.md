---
source: caa4681b9da88e6c8635967bd981136c01a89951
---

# Terrain { #terrain }

Ein Terrain ist ein großes Raster aus Höhen, das du wie Ton modellierst, mit bis zu vier darübergemalten Texturen. Nimm es für offenes Gelände wie Hügel, Strände und Täler, wo es ewig dauern würde, Brushes von Hand zu formen.

Diese Seite zeigt, wie du eins anlegst und mit dem Modellier-Werkzeug (G) formst. Das Texturieren steht auf [Terrainebenen](terrain-layers.md) und [Terrain malen](terrain-painting.md).

## Ein Terrain anlegen { #creating-a-terrain }

1. Den Dialog öffnen

    Geh in der Menüleiste auf *Terrain > Terrain erstellen…*.

    ![Das Menü Terrain geöffnet, Terrain erstellen… mit dem Mauszeiger darauf](../assets/terrain/menu-create-terrain.png)

2. Eine Größe wählen

    **Auflösung** und **Zellgröße** bestimmen, wie groß es wird, und die Zeile **Größe** darunter zeigt das Ergebnis in Einheiten und Metern. Die Vorgabe von 129 x 129 Vertices mit Zellen von 64 Einheiten ergibt 8192 Einheiten, 256 m. Siehe [Die Zellgröße wählen](#picking-the-cell-size) weiter unten.

    ![Der Dialog Terrain erstellen mit seinen Vorgaben: 129 x 129, Zellen mit 64 u, Hügel und die vier Ebenenfelder](../assets/terrain/create-terrain-dialog.png)

3. (Optional) Eine Startform wählen

    **Form** ist der Untergrund, mit dem du anfängst. Vorgabe ist Hügel, die anderen sind Flach, Berg, Insel, Tal und Grate. **PNG importieren…** liest stattdessen eine Graustufen-Höhenkarte ein, bei der hellere Pixel höher liegen. Beides geht nur hier, beim Anlegen.

4. (Optional) Die Ebenen wählen

    **Basisebene**, **Ebene für Hänge**, **Ebene für Gipfel** und **Ebene für Senken** sind die vier Texturen des Terrains, und **Aktuelles nehmen** setzt das Material ein, auf das du zuletzt geklickt hast. Ist **Ebenen nach Neigung und Höhe bemalen** angehakt, werden sie sofort bemalt, siehe [Automatisch malen](terrain-painting.md#auto-paint).

5. Unten auf **Erstellen** klicken

    Das Terrain wird am 3D-Cursor zentriert, oder an der **Mitte**, die du eintippst, wenn **3D-Cursor** nicht angehakt ist, und diese Höhe wird zur Null des Terrains. Du solltest das neue Terrain ausgewählt sehen, das Modellier-Werkzeug ist schon aktiv.

## Die Zellgröße wählen { #picking-the-cell-size }

| Einstellung | Bedeutung |
| --- | --- |
| Auflösung | Wie viele Vertices an jeder Seite entlanglaufen, von 33 bis 1025. Mehr Vertices halten mehr Details, kosten aber mehr beim Zeichnen und Modellieren |
| Zellgröße | Der Abstand in Karteneinheiten zwischen zwei benachbarten Vertices |

257 Vertices mit Zellen von 64 Einheiten ergeben zum Beispiel 16384 Einheiten, 512 m.

Zwischen zwei Vertices liegt nichts, die Zellgröße ist also das feinste Detail, das das Terrain halten kann. Ein Gebirge braucht große Zellen, ein Garten kleine.

Falsch gewählt? Du kannst [die Größe ändern](#changing-the-size), jederzeit nach dem Anlegen des Terrains.

**Chunk-Zellen** (standardmäßig 32) teilt das Terrain zum Zeichnen und für die Kollision in quadratische Stücke. Kleinere Chunks bauen sich beim Modellieren schneller neu auf, größere bedeuten weniger Nodes in Godot.

## Die Größe ändern { #changing-the-size }

Ein Terrain bleibt nicht auf der Größe, mit der du es angelegt hast. Du kannst an jeder Seite Zellen anfügen, sie abschneiden oder die Zellgröße ändern, und der Boden, den du schon modelliert hast, bleibt genau da, wo er in der Karte war.

1. Wähl das Terrain aus

    Klick es in einer Ansicht oder im Outliner an. Sein Block **Größe** sitzt im Inspektor unter **Ursprung**.

2. Gib die Änderung ein

    **Zellen** zeigt, wie viele Zellen entlang X und Z laufen. Gib dort eine Gesamtzahl ein, dann wachsen oder schrumpfen beide gegenüberliegenden Seiten gleichmäßig, und die ungerade Zelle geht an die Ost- oder Südseite. Die Felder **Norden (-Z)**, **Süden (+Z)**, **Westen (-X)** und **Osten (+X)** ändern nur eine Seite. Eine positive Zahl fügt so viele Zellen an und eine negative entfernt sie, `+8` bei Osten fügt also 8 Zellen an und `-2` bei Süden schneidet 2 ab.

    ![Der Block Größe mit 8 Zellen mehr im Osten und 2 weniger im Süden, darunter die Zeile Danach mit dem Ergebnis](../assets/terrain/inspector-size.png)

    Noch ändert sich nichts. Deine Eingaben sind ein Entwurf, als Umriss in den Ansichten gezeichnet, und die Zeile **Danach** unter **Anwenden** sagt dir, welche Zellen und welche Größe du bekämst.

3. Wähl, was der neue Boden bekommt

    **Neuer Boden** erscheint, sobald eine Seite wächst. **Randhöhe** setzt den nächsten Rand fort, Höhe und Bemalung, das Terrain wird also einfach länger. **Null** gibt den neuen Zellen die Höhe 0 und die erste Ebene, als flache Fläche zum Bauen. Die erste neue Zelle fällt trotzdem vom alten Rand auf 0 ab, die flache Fläche beginnt also eine Zelle weiter draußen.

4. Klick auf **Anwenden**

    Das Terrain ändert sich in einem Schritt, und Ctrl+Z stellt es genau wieder her. **Zurücksetzen** verwirft stattdessen den Entwurf.

Alles, was bleibt, liegt an derselben Weltposition wie vorher: Höhen, Bemalung und Löcher der Zellen, die bleiben. Fügst du an der West- oder Nordseite Zellen an, wandert der **Ursprung** des Terrains passend nach außen.

**Zellgröße** funktioniert anders. Sie behält die Größe des Terrains in der Karte auf die nächste ganze Zelle genau und baut das Raster mit dem neuen Abstand neu auf, die Zellenzahl ändert sich also mit, und Höhen und Bemalung werden zwischen den alten Vertices gemischt. Ist die Größe kein Vielfaches der neuen Zelle, wächst oder schrumpft das Terrain um bis zu eine halbe Zelle, und **Ganze Zellen ändern die Größe um** unter **Danach** sagt dir, um wie viel. Die Zellgröße zurückzustellen macht dieses Runden nicht rückgängig, Ctrl+Z schon. Größere Zellen werfen Details weg, und der Inspektor warnt dich vor dem Anwenden mit **Feine Details gehen verloren**. Kleinere Zellen behalten die Form, können aber keine Details hinzufügen, die nie da waren. Erst gelten die Seiten, dann die Zellgröße.

> [!NOTE]
> Ein Terrain hat entlang jeder Achse zwischen 1 und 2048 Zellen und eine Zellgröße zwischen 0.001 und 100000 Einheiten. Eine Änderung, die darüber hinausgeht oder jede Zelle abschneiden würde, wird abgelehnt, der Grund steht in Rot, und **Anwenden** bleibt ausgegraut.

### Scatter auf dem Terrain { #scatter-on-the-terrain }

Auf das Terrain gemalter [Scatter](scatter.md) bleibt am Boden. Instanzen über Boden, der noch da ist, steigen oder sinken mit der neuen Höhe, nichts schwebt also oder versinkt. Instanzen über abgeschnittenem Boden werden standardmäßig entfernt, und wenn es welche gibt, erscheint unter **Anwenden** ein Kästchen **n Scatter-Instanzen auf entfernter Fläche entfernen**. Nimm den Haken heraus, damit sie bleiben, wo sie sind, schwebend über nichts, wenn du sie selbst verschieben willst. Ein Scatter-Set, das gesperrt oder ausgeblendet ist oder in einer gesperrten oder ausgeblendeten Ebene liegt, bleibt, wie es ist.

## Modellieren { #sculpting }

Drück G, wähl links oben in der Optionsleiste des Werkzeugs einen Modus und zieh über das Terrain. Es wirkt auf die ausgewählten Terrains, oder auf alle, wenn nichts ausgewählt ist.

![Das Modusmenü des Modellier-Werkzeugs geöffnet in der Optionsleiste, neben den Feldern für Radius und Stärke](../assets/terrain/sculpt-modes.png)

Halt die Maustaste gedrückt, dann steigt der Boden unter dem Brush weiter an.

![Mit gehaltenem Anheben und einem Brush von 900 Einheiten wächst aus der Insel ein Hügel](../assets/terrain/sculpt-raise.gif)

| Modus | Was er tut |
| --- | --- |
| Anheben, Absenken | Baut Höhe auf oder gräbt sie ab, solange du die Taste hältst. Ein Klick macht eine kleine Beule |
| Glätten | Entspannt jeden Vertex zu seinen Nachbarn hin, um Klumpen und scharfe Grate weicher zu machen |
| Einebnen | Zieht den Boden auf die Höhe, in der der Strich begann, für Plattformen, Straßen und Plateaus |
| Rauschen | Fügt kleine zufällige Beulen hinzu, damit ebener Boden weniger künstlich aussieht |
| Terrassieren | Rastet Höhen auf Stufen von **Schritt** Einheiten ein, für Reisfelder oder abgestufte Klippen. Das Feld erscheint neben dem Modus |
| Alpha malen, Alpha löschen, Ebene malen | Malen statt formen. Die Alpha-Modi sind für Displacements gedacht, Ebene malen malt die Terrainebene, die neben dem Modus eingestellt ist. Auf einem Terrain macht das das [Misch-Werkzeug](terrain-painting.md) mit mehr Kontrolle |
| Loch, Loch füllen | Schneidet ganze Zellen heraus oder füllt sie wieder, Kollision eingeschlossen. Nimm es für Höhlen- oder Bunkereingänge |

Ctrl+Mausrad ändert die Brush-Größe, Shift tauscht Anheben und Absenken, und gehaltenes Ctrl glättet, in welchem Modus auch immer.

**Stärke** legt fest, wie schnell der Brush arbeitet. Anheben und Absenken bewegen die Mitte des Brushs pro Punkt Stärke 8 Einheiten in der Sekunde, egal bei welchem Radius, die Vorgabe 4 baut also 32 Einheiten in der Sekunde auf. Für Hügel und Inseln dreh lieber die Stärke hoch, statt eine Minute lang zu halten.

Halt den Radius über der Zellgröße des Terrains, ein kleinerer Brush stupst nur den einen Vertex unter sich an (die Statusleiste sagt dir, wenn das passiert).

Um neu anzufangen, setzt *Terrain > Modellieren > Einebnen* oder **Einebnen** im Inspektor das ganze ausgewählte Terrain auf seine Nullhöhe zurück.

> [!TIP]
> Forme mit einem großen Brush und glätte nach jedem groben Durchgang. Kleine Brushes hinterlassen Klumpen.

## In Godot { #in-godot }

Jedes Terrain wird zu einem `TrenchDotTerrain`-Node, einem `StaticBody3D` (Godots Node für festen Boden, der sich nie bewegt), dessen Kollision dem entspricht, was er zeichnet. Für Gameplay-Code liefert `height_at(x, z)` die Bodenhöhe an einer lokalen Position, praktisch, um zur Laufzeit Dinge auf den Boden zu setzen.

<div class="td-mcp" markdown>

## MCP { #mcp }

Die Operation `sculpt` des Tools `terrain_edit` wendet einen einzelnen Tupfer an, und seine Stärke ist, wie weit dieser Tupfer die Mitte bewegt, wieder unabhängig vom Radius. Einen Strich eine Sekunde lang bei Stärke 4 zu halten entspricht einem Tupfer der Stärke 32.

Dieselbe Größenänderung ist `terrain_edit` mit `op: "resize"` und nimmt `north`, `south`, `west` und `east` in Zellen oder `cells: [x, z]`, dazu `cell_size`. Mit `dry_run: true` liest du vorher die Zellen, Grenzen, `scatter_removed` und `detail_lost` des Ergebnisses. `fill` ist `edge` oder `zero` und `scatter` ist `drop` oder `keep`, wie im Inspektor. Die Grenzen stehen unter [MCP](../mcp.md).

</div>
