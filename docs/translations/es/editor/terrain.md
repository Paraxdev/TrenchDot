---
source: e038948c3cf9dabdef8c6212475c2c269d817346
---

# Terreno { #terrain }

Un terreno es una gran rejilla de alturas que esculpes como arcilla, con hasta cuatro texturas pintadas encima. Úsalo para suelo abierto como colinas, playas y valles, donde dar forma a mano con brushes llevaría una eternidad.

Esta página cubre cómo crear uno y darle forma con la herramienta Esculpir (G). Texturizarlo está en [Capas de terreno](terrain-layers.md) y [Pintar terreno](terrain-painting.md).

Un terreno puede crecer después de crearlo, hasta 2048 celdas a lo largo de cada eje, ya sea con el bloque **Tamaño** del Inspector ([Cambiar el tamaño](#changing-the-size)) o arrastrando su borde en una vista 2D con la herramienta Seleccionar ([Arrastrar un borde](#dragging-an-edge)). Admite hasta cuatro capas, que pintas con la [herramienta Mezcla](terrain-painting.md).

Como el suelo es un campo de alturas, no puede plegarse sobre sí mismo, mira [Cuevas y voladizos](caves-overhangs.md) para saber qué hacer en su lugar.

## Crear un terreno { #creating-a-terrain }

1. Abre el diálogo

    Ve a *Terreno > Crear terreno…* en la barra de menús.

    ![El menú Terreno abierto con Crear terreno… bajo el cursor](../assets/terrain/menu-create-terrain.png)

2. Elige un tamaño

    **Resolución** y **Tamaño de celda** deciden lo grande que queda, y la fila **Tamaño** bajo ellos muestra el resultado en unidades y metros. Los valores por defecto, 129 x 129 vértices con celdas de 64 unidades, dan 8192 unidades, 256 m. Mira [Elegir el tamaño de celda](#picking-the-cell-size) más abajo.

    ![El diálogo Crear terreno con sus valores por defecto: 129 x 129, celdas de 64 u, Colinas y los cuatro campos de capa](../assets/terrain/create-terrain-dialog.png)

3. (Opcional) Elige una forma inicial

    **Forma** es el suelo del que partes. Por defecto es Colinas, las demás son Plano, Montaña, Isla, Valle y Crestas. Las filas de debajo ajustan el ruido con el que se genera:

    | Fila | Significado |
    | --- | --- |
    | Altura | Cuánto sube el suelo, 512 u por defecto |
    | Tamaño de rasgos | Qué ancha es una colina o un valle, 2048 u por defecto |
    | Octavas / rugosidad | Más octavas y más rugosidad añaden bultos más finos encima |
    | Erosión | Cuántas pasadas dejan que las pendientes empinadas se desmoronen, 20 por defecto |
    | Semilla | La misma semilla da otra vez el mismo suelo |

    **Importar PNG…** junto a **Mapa de alturas** lee en su lugar una imagen en escala de grises (PNG, TGA o BMP), donde los píxeles más claros son más altos, hasta **Altura**. **Borrar** vuelve a la forma generada. La forma y el mapa de alturas solo valen aquí, al crear.

4. (Opcional) Elige las capas

    **Capa base**, **Capa de pendiente**, **Capa de cima** y **Capa baja** son las cuatro texturas del terreno, y empiezan como `dev/green`, `dev/grey`, `dev/blue` y `dev/orange`. **Usar actual** pone el material en el que hiciste clic por última vez, y **mosaico** es cuántas unidades cubre una repetición. **Pintar capas según pendiente y altura** viene marcada por defecto, pinta las capas al momento, mira [Pintado automático](terrain-painting.md#auto-paint).

5. Haz clic en **Crear** abajo del todo

    El terreno se centra en el cursor 3D, o en el **Centro** que escribes una vez que desmarcas **Cursor 3D**, y esa altura pasa a ser el cero del terreno. Deberías ver el terreno nuevo seleccionado, con la herramienta Esculpir ya activa.

## Elegir el tamaño de celda { #picking-the-cell-size }

| Ajuste | Significado |
| --- | --- |
| Resolución | Cuántos vértices hay a lo largo de cada lado, uno de 33, 65, 129, 257, 513 o 1025. Más vértices guardan más detalle pero cuestan más al dibujar y esculpir |
| Tamaño de celda | La distancia en unidades del mapa entre dos vértices vecinos, de 4 a 1024 |

257 vértices con celdas de 64 unidades, por ejemplo, dan 16384 unidades, 512 m.

No hay nada entre dos vértices, así que el tamaño de celda es el detalle más fino que puede guardar el terreno. Una cordillera pide celdas grandes, un jardín celdas pequeñas.

¿Te equivocaste? Puedes [cambiar el tamaño](#changing-the-size) cuando quieras después de crear el terreno.

**Celdas por chunk** en el Inspector (32 por defecto) divide el terreno en piezas cuadradas para dibujar y para la colisión. Los chunks pequeños se reconstruyen más rápido al esculpir, los grandes significan menos nodos en Godot.

## Cambiar el tamaño { #changing-the-size }

Un terreno no se queda atascado en el tamaño con que lo creaste. Puedes añadir celdas en cualquier lado, recortarlas o cambiar el tamaño de celda, y el suelo que ya esculpiste se queda justo donde estaba en el mapa.

1. Selecciona el terreno

    Haz clic en él en una vista o en el Outliner. Su bloque **Tamaño** está en el Inspector bajo **Origen**.

2. Escribe el cambio

    | Campo | Qué hace escribir ahí |
    | --- | --- |
    | **Celdas** | Fija el total a lo largo de X y Z. Los dos lados opuestos crecen o se encogen por igual, y la celda sobrante va al lado este o sur |
    | **Norte (-Z)**, **Sur (+Z)**, **Oeste (-X)**, **Este (+X)** | Cambia solo ese lado. Un número positivo añade esas celdas y uno negativo las quita, así que `+8` en Este añade 8 y `-2` en Sur recorta 2 |

    ![El bloque Tamaño con 8 celdas añadidas al este y 2 quitadas al sur, y la línea Después mostrando el resultado](../assets/terrain/inspector-size.png)

    Todavía no cambia nada. Tus ediciones son un borrador, dibujado como contorno en las vistas, y la línea **Después** bajo **Aplicar** te dice las celdas y el tamaño que tendrías.

3. Elige qué recibe el suelo nuevo

    **Suelo nuevo** aparece en cuanto un lado crece. **Altura del borde** continúa el borde más cercano, tanto la altura como la pintura, así que el terreno simplemente se alarga. **Cero** da a las celdas nuevas altura 0 y la primera capa, para una franja plana donde construir. La primera celda nueva sigue bajando del borde viejo hasta 0, así que la parte plana empieza una celda más afuera.

4. Haz clic en **Aplicar**

    El terreno cambia en un solo paso, y Ctrl+Z lo devuelve exactamente. **Restablecer** descarta el borrador en su lugar.

Todo lo que se conserva aparece en la misma posición del mundo que antes: alturas, pintura y agujeros de las celdas que quedan. Si añades celdas en el lado oeste o norte, el **Origen** del terreno se mueve hacia fuera para cuadrar.

**Tamaño de celda** funciona distinto, reconstruye la cuadrícula con el nuevo espaciado:

* El terreno conserva su tamaño en el mapa, redondeado a la celda entera más cercana, así que el número de celdas cambia con él. Las alturas y la pintura se mezclan entre los vértices antiguos.
* Cuando el tamaño no es múltiplo de la celda nueva, el terreno crece o se encoge hasta media celda, y **Las celdas enteras cambian el tamaño en** bajo **Después** te dice cuánto. Volver a poner el tamaño de celda no deshace ese redondeo, Ctrl+Z sí.
* Las celdas más grandes tiran detalle, y el Inspector te avisa con **Se pierde detalle fino** antes de aplicar. Las celdas más pequeñas conservan la forma pero no pueden añadir detalle que nunca existió.
* Primero se aplican los lados, luego el tamaño de celda.

> [!NOTE]
> Un terreno tiene entre 1 y 2048 celdas a lo largo de cada eje, y un tamaño de celda entre 0.001 y 100000 unidades. Un cambio que se sale de eso, o que recortaría todas las celdas, se rechaza con el motivo en rojo, y **Aplicar** queda en gris.

### Arrastrar un borde { #dragging-an-edge }

También puedes cambiar el tamaño directamente en las vistas **Arriba**, **Frontal** y **Lateral**, siempre que haya exactamente un terreno seleccionado, sin bloquear y sin caras seleccionadas. Con la herramienta Seleccionar, pasa el puntero por uno de sus bordes hasta que el cursor se vuelva una flecha de tamaño y el borde se ilumine, y arrástralo hacia fuera para añadir celdas o hacia dentro para recortarlas. El borde salta a celdas enteras y la barra de estado muestra el nuevo número de celdas. Cada arrastre es un paso que Ctrl+Z deshace.

![Un terreno en la vista Superior con su borde este arrastrado cuatro celdas hacia fuera](../assets/terrain/edge-drag.png)

Un arrastre funciona como escribir en el campo de ese lado y hacer clic en **Aplicar** al instante. El suelo nuevo continúa el borde, como hace **Altura del borde**, y un terreno nunca baja de 1 celda.

En las vistas Frontal y Lateral solo agarran los extremos izquierdo y derecho, porque la altura del suelo sale de esculpir. Escribe mejor en el bloque **Tamaño** cuando quieras suelo **Cero** o un tamaño de celda nuevo.

### El scatter sobre el terreno { #scatter-on-the-terrain }

El [Scatter](scatter.md) pintado en el terreno se queda en el suelo cuando cambias su tamaño:

* Las instancias sobre suelo que sigue ahí suben o bajan con la nueva altura, así que nada flota ni se hunde.
* Las instancias sobre suelo recortado se quitan por defecto. Cuando hay alguna, aparece bajo **Aplicar** una casilla **Quitar n instancias de scatter del suelo quitado**. Desmárcala para dejarlas donde están, flotando sobre nada, si piensas moverlas tú.
* Arrastrar un borde hacia dentro siempre las quita, y Ctrl+Z las devuelve.
* Un conjunto de scatter bloqueado u oculto, o en una capa bloqueada u oculta, se queda como está.

## Esculpir { #sculpting }

Pulsa G, elige un modo en el primer menú de la barra de opciones de la herramienta, arriba a la izquierda, y arrastra sobre el terreno. Funciona sobre los terrenos seleccionados, o sobre todos cuando no hay ninguno seleccionado.

![El menú de modos de la herramienta Esculpir abierto en la barra de opciones, junto a los campos de radio y fuerza](../assets/terrain/sculpt-modes.png)

Con Elevar, mantén el botón del ratón pulsado y el suelo sigue subiendo bajo el pincel.

![Mantener Elevar con un pincel de 900 unidades levanta una colina en la isla](../assets/terrain/sculpt-raise.gif)

| Modo | Qué hace |
| --- | --- |
| Elevar, Bajar | Sube altura o la cava mientras mantengas el botón. Un clic hace un bultito |
| Suavizar | Relaja cada vértice hacia sus vecinos, para suavizar bultos y crestas afiladas |
| Aplanar | Lleva el suelo a la altura donde empezó el trazo, para hacer explanadas, caminos y mesetas |
| Ruido | Añade pequeños bultos aleatorios, para que el suelo plano parezca menos artificial |
| Aterrazar | Ajusta las alturas a escalones de **paso** unidades (32 por defecto), para arrozales o acantilados escalonados. El campo aparece junto al modo |
| Pintar alfa, Borrar alfa, Pintar capa | Pintan en lugar de dar forma. Los modos alfa están pensados para displacements, Pintar capa pinta la capa del terreno que fijas en el campo **capa** junto al modo. En un terreno la [herramienta Mezcla](terrain-painting.md) hace esto con más control |
| Agujero, Tapar agujero | Recorta celdas enteras o las rellena, colisión incluida. Úsalo para entradas de cuevas o búnkeres, mira [Cuevas y voladizos](caves-overhangs.md) |

Ctrl+rueda cambia el tamaño del pincel, Shift intercambia elevar y bajar, y mantener Ctrl suaviza sea cual sea el modo.

El campo **fuerza** de la barra de opciones fija lo rápido que trabaja el pincel. Elevar y Bajar mueven el centro del pincel 8 unidades por segundo por punto de fuerza, sea cual sea el **radio**, así que el 4 por defecto sube 32 unidades por segundo. Para colinas e islas sube la fuerza en vez de mantener pulsado un minuto.

Mantén el radio por encima del tamaño de celda del terreno, un pincel más pequeño solo empuja el vértice que tiene debajo (la barra de estado te avisa cuando pasa).

Para empezar de cero, *Terreno > Esculpir > Aplanar*, o **Aplanar** en el Inspector, devuelve todo el terreno seleccionado a su altura cero.

> [!TIP]
> Da forma con un pincel grande y suaviza después de cada pasada brusca. Los pinceles pequeños dejan bultos.

## Simetría { #symmetry }

Para esculpir los dos lados de un valle o de una isla a la vez, activa la simetría. Las herramientas **Esculpir** y **Mezcla** tienen el control en la barra de opciones, justo después de **fuerza** y de los campos del modo.

![El menú de simetría abierto en la barra de opciones de la herramienta Esculpir, con desactivada, X, Z y X y Z](../assets/terrain/symmetry-menu.png)

| Control | Qué hace |
| --- | --- |
| Menú **simetría desactivada** | Elige **X** para reflejar las mitades oeste y este una en otra, **Z** para norte y sur, o **X y Z** para ambas. El menú cerrado dice entonces **simetría X**, **simetría Z** o **simetría X y Z** |
| Menú **centro del terreno** | Solo aparece con la simetría activada. Decide dónde están los planos de espejo: **centro del terreno** (el medio de cada terreno), **origen del mundo** o **personalizado** |
| Campos **x** y **z** | Solo con **personalizado**, uno por eje reflejado, la posición del plano en unidades. Empiezan en los planos que acabas de usar, así que nada salta |

Con la simetría activada, las vistas dibujan los planos mientras Esculpir o Mezcla es la herramienta activa, como un contorno rojo para X y uno azul para Z. Una línea sigue el suelo allí donde cada plano lo corta, y una copia más tenue del anillo del pincel muestra dónde caerán los toques reflejados antes de que pulses.

![La vista 3D con simetría X y Z, mostrando los dos planos de espejo y el anillo del pincel repetido en los cuatro cuadrantes](../assets/terrain/symmetry-planes.png)

Arrastra como siempre. Cada toque se aplica en el cursor y en sus puntos reflejados, y el trazo entero sigue siendo un solo paso de deshacer. Un toque sobre un plano, o uno que se solapa con su propio reflejo, cuenta una vez, así que el suelo no sube el doble allí. El ruido, el ruido de mezcla y el spray salen como una imagen en espejo exacta en vez de un patrón aleatorio nuevo.

Con **centro del terreno** cada terreno se refleja sobre su propio centro, y los displacements se reflejan sobre el centro de todos juntos. Esculpir y Mezcla comparten el ajuste. Cada pestaña de mapa guarda el suyo, un mapa nuevo o abierto empieza con el que estabas usando pero pasa de **personalizado** a **centro del terreno**, y vuelve a empezar desactivado cada vez que abres el editor.

> [!TIP]
> Para reflejar entre varios terrenos colocados uno junto a otro, o alrededor de un punto que no es su centro, usa **origen del mundo** o **personalizado**. Pon el plano en una línea de vértices o justo a medio camino entre dos para una coincidencia exacta, cualquier otra posición solo refleja de forma aproximada.

<div class="td-mcp" markdown>

La acción `sculpt`, la herramienta `blend` y `terrain_edit` (`sculpt`, `sculpt_path`, `paint_path` y `holes` con `center`) aceptan un argumento `symmetry`, `off`, `x`, `z`, `xz` o un objeto `{axes, center, x, z}`. `set_editor` fija el que usa la barra de herramientas y `get_state` lo informa, mira [MCP](../mcp.md).

</div>

## En Godot { #in-godot }

Cada terreno se convierte en un nodo `TrenchDotTerrain`, un `StaticBody3D` (el nodo de Godot para suelo sólido que nunca se mueve) cuya colisión coincide con lo que dibuja. Para el código de juego, `height_at(x, z)` devuelve la altura del suelo en una posición local, útil para colocar cosas en el suelo en tiempo de ejecución.

<div class="td-mcp" markdown>

## MCP { #mcp }

La operación `sculpt` de la herramienta `terrain_edit` aplica un solo toque, y su fuerza es cuánto mueve ese toque el centro, otra vez sea cual sea el radio. Mantener un trazo un segundo con fuerza 4 equivale a un toque de fuerza 32.

El mismo cambio de tamaño es `terrain_edit` con `op: "resize"`, que toma `north`, `south`, `west` y `east` en celdas, o `cells: [x, z]`, y `cell_size`. Añade `dry_run: true` para leer antes las celdas, los límites, `scatter_removed` y `detail_lost` del resultado. `fill` es `edge` o `zero` y `scatter` es `drop` o `keep`, como en el Inspector. Mira [MCP](../mcp.md) para los límites.

</div>
