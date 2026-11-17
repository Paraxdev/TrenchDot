---
source: e038948c3cf9dabdef8c6212475c2c269d817346
---

# Terreno { #terrain }

Um terreno é uma grade grande de alturas que você esculpe como argila, com até quatro texturas pintadas por cima. Use-o para chão aberto, como colinas, praias e vales, onde moldar brushes à mão levaria uma eternidade.

Esta página cobre como criar um e dar forma a ele com a ferramenta Esculpir (G). Texturizá-lo está em [Camadas de terreno](terrain-layers.md) e [Pintura de terreno](terrain-painting.md).

Um terreno pode crescer depois de criado, até 2048 células ao longo de cada eixo, seja com o bloco **Tamanho** do Inspetor ([Mudando o tamanho](#changing-the-size)) ou arrastando a borda dele em uma vista 2D com a ferramenta Selecionar ([Arrastando uma borda](#dragging-an-edge)). Ele guarda até quatro camadas, que você pinta com a [ferramenta Mistura](terrain-painting.md).

Como o chão é um campo de alturas, ele não consegue se dobrar sobre si mesmo, veja [Cavernas e saliências](caves-overhangs.md) para saber o que fazer no lugar.

## Criando um terreno { #creating-a-terrain }

1. Abra a janela

    Vá em *Terreno > Criar terreno…* na barra de menus.

    ![O menu Terreno aberto com Criar terreno… sob o mouse](../assets/terrain/menu-create-terrain.png)

2. Escolha um tamanho

    **Resolução** e **Tamanho da célula** decidem o tamanho que ele terá, e a linha **Tamanho** embaixo delas mostra o resultado em unidades e metros. O padrão de 129 x 129 vértices com células de 64 unidades dá 8192 unidades, 256 m. Veja [Escolhendo o tamanho da célula](#picking-the-cell-size) abaixo.

    ![A janela Criar terreno com os padrões: 129 x 129, células de 64 u, Colinas e os quatro campos de camada](../assets/terrain/create-terrain-dialog.png)

3. (Opcional) Escolha uma forma inicial

    **Forma** é o chão de onde você parte. O padrão é Colinas, as outras são Plano, Montanha, Ilha, Vale e Cristas. As linhas abaixo ajustam o ruído de que ele é feito:

    | Linha | Significado |
    | --- | --- |
    | Altura | Quão alto o chão fica, 512 u por padrão |
    | Tamanho dos detalhes | Quão larga é uma colina ou um vale, 2048 u por padrão |
    | Oitavas / rugosidade | Mais oitavas e uma rugosidade maior acrescentam calombos mais finos por cima |
    | Erosão | Quantas passadas deixam os declives íngremes desabar, 20 por padrão |
    | Semente | A mesma semente dá o mesmo chão de novo |

    **Importar PNG…** ao lado de **Mapa de altura** lê no lugar uma imagem em tons de cinza (PNG, TGA ou BMP), onde pixels mais claros são mais altos, até **Altura**. **Limpar** volta à forma gerada. A forma e o mapa de altura só valem aqui, na criação.

4. (Opcional) Escolha as camadas

    **Camada base**, **Camada de declive**, **Camada de pico** e **Camada baixa** são as quatro texturas do terreno, e começam como `dev/green`, `dev/grey`, `dev/blue` e `dev/orange`. **Usar atual** coloca o material em que você clicou por último, e **ladrilho** é quantas unidades uma repetição dele cobre. **Pintar camadas por declive e altura** vem marcada por padrão, ela pinta as camadas na hora, veja [Pintura automática](terrain-painting.md#auto-paint).

5. Clique em **Criar** na parte de baixo

    O terreno é centralizado no cursor 3D, ou no **Centro** que você digita depois de desmarcar **Cursor 3D**, e essa altura vira o zero do terreno. Você deve ver o terreno novo selecionado, com a ferramenta Esculpir já ativa.

## Escolhendo o tamanho da célula { #picking-the-cell-size }

| Configuração | Significado |
| --- | --- |
| Resolução | Quantos vértices correm ao longo de cada lado, um entre 33, 65, 129, 257, 513 ou 1025. Mais vértices guardam mais detalhe, mas custam mais para desenhar e esculpir |
| Tamanho da célula | A distância em unidades do mapa entre dois vértices vizinhos, de 4 a 1024 |

257 vértices com células de 64 unidades, por exemplo, dão 16384 unidades, 512 m.

Nada fica entre dois vértices, então o tamanho da célula é o detalhe mais fino que o terreno consegue guardar. Uma cordilheira pede células grandes, um jardim pede pequenas.

Escolheu errado? Você pode [mudar o tamanho](#changing-the-size) a qualquer momento depois de criar o terreno.

**Células por chunk** no Inspetor (32 por padrão) divide o terreno em pedaços quadrados para desenho e colisão. Chunks menores se refazem mais rápido ao esculpir, maiores significam menos nodes no Godot.

## Mudando o tamanho { #changing-the-size }

Um terreno não fica preso no tamanho com que você o criou. Você pode adicionar células em qualquer lado, cortá-las ou mudar o tamanho da célula, e o solo que você já esculpiu fica exatamente onde estava no mapa.

1. Selecione o terreno

    Clique nele em uma vista ou no Outliner. O bloco **Tamanho** dele fica no Inspetor, abaixo de **Origem**.

2. Digite a mudança

    | Campo | O que digitar ali faz |
    | --- | --- |
    | **Células** | Define o total ao longo de X e Z. Os dois lados opostos crescem ou encolhem por igual, com a célula ímpar indo para o lado leste ou sul |
    | **Norte (-Z)**, **Sul (+Z)**, **Oeste (-X)**, **Leste (+X)** | Muda só esse lado. Um número positivo adiciona essa quantidade de células e um negativo as remove, então `+8` em Leste adiciona 8 e `-2` em Sul corta 2 |

    ![O bloco Tamanho com 8 células adicionadas a leste e 2 removidas ao sul, e a linha Depois mostrando o resultado](../assets/terrain/inspector-size.png)

    Nada muda ainda. Suas edições são um rascunho, desenhado como contorno nas vistas, e a linha **Depois** abaixo de **Aplicar** diz as células e o tamanho que você teria.

3. Escolha o que o solo novo recebe

    **Solo novo** aparece assim que um lado cresce. **Altura da borda** continua a borda mais próxima, tanto a altura quanto a pintura, então o terreno simplesmente fica mais longo. **Zero** dá às células novas altura 0 e a primeira camada, para uma faixa plana onde construir. A primeira célula nova ainda desce da borda antiga até 0, então a parte plana começa uma célula para fora.

4. Clique em **Aplicar**

    O terreno muda em um único passo, e Ctrl+Z o devolve exatamente. **Redefinir** descarta o rascunho em vez disso.

Tudo o que foi mantido aparece na mesma posição do mundo de antes: alturas, pintura e buracos das células que ficam. Se você adicionar células no lado oeste ou norte, a **Origem** do terreno se move para fora para acompanhar.

**Tamanho da célula** funciona diferente, ele refaz a grade com o novo espaçamento:

* O terreno mantém o tamanho dele no mapa, até a célula inteira mais próxima, então o número de células muda junto. Alturas e pintura são misturadas entre os vértices antigos.
* Quando o tamanho não é múltiplo da célula nova, o terreno cresce ou encolhe até meia célula, e **Células inteiras mudam o tamanho em** abaixo de **Depois** diz quanto. Voltar o tamanho da célula não desfaz esse arredondamento, Ctrl+Z sim.
* Células maiores jogam detalhe fora, e o Inspetor avisa com **Detalhes finos se perdem** antes de você aplicar. Células menores mantêm a forma, mas não conseguem acrescentar detalhe que nunca existiu.
* Os lados valem primeiro, depois o tamanho da célula.

> [!NOTE]
> Um terreno tem entre 1 e 2048 células ao longo de cada eixo, e um tamanho de célula entre 0.001 e 100000 unidades. Uma mudança que saia disso, ou que cortaria todas as células, é recusada com o motivo em vermelho, e **Aplicar** fica cinza.

### Arrastando uma borda { #dragging-an-edge }

Você também pode mudar o tamanho direto nas vistas **Topo**, **Frente** e **Lado**, desde que exatamente um terreno esteja selecionado, destravado e sem faces selecionadas. Com a ferramenta Selecionar, passe o ponteiro sobre uma das bordas dele até o cursor virar uma seta de redimensionar e a borda acender, e arraste-a para fora para adicionar células ou para dentro para cortá-las. A borda encaixa em células inteiras e a barra de status mostra o novo número de células. Cada arrasto é um passo que Ctrl+Z desfaz.

![Um terreno na vista Topo com a borda leste arrastada quatro células para fora](../assets/terrain/edge-drag.png)

Um arrasto funciona como digitar no campo daquele lado e clicar em **Aplicar** na hora. O chão novo continua a borda, como **Altura da borda** faz, e um terreno nunca fica com menos de 1 célula.

Nas vistas Frente e Lado só as pontas esquerda e direita pegam, já que a altura do chão vem de esculpir. Digite no bloco **Tamanho** quando quiser chão **Zero** ou um tamanho de célula novo.

### Scatter no terreno { #scatter-on-the-terrain }

O [scatter](scatter.md) pintado no terreno fica no chão quando você muda o tamanho dele:

* Instâncias sobre solo que continua lá sobem ou descem com a nova altura, então nada flutua nem afunda.
* Instâncias sobre solo cortado são removidas por padrão. Quando existe alguma, uma caixa **Remover n instâncias de scatter do solo removido** aparece abaixo de **Aplicar**. Desmarque-a para mantê-las onde estão, flutuando sobre o nada, se você pretende movê-las por conta própria.
* Arrastar uma borda para dentro sempre as remove, e Ctrl+Z as traz de volta.
* Um conjunto de scatter travado ou oculto, ou numa camada travada ou oculta, fica como está.

## Esculpindo { #sculpting }

Aperte G, escolha um modo no primeiro menu da barra de opções da ferramenta no canto superior esquerdo e arraste sobre o terreno. Ela age nos terrenos selecionados, ou em todos eles quando nada está selecionado.

![O menu de modos da ferramenta Esculpir aberto na barra de opções da ferramenta, ao lado dos campos de raio e força](../assets/terrain/sculpt-modes.png)

Com Elevar, mantenha o botão do mouse pressionado e o chão continua subindo sob o brush.

![Segurando Elevar com um brush de 900 unidades, uma colina se forma na ilha](../assets/terrain/sculpt-raise.gif)

| Modo | O que faz |
| --- | --- |
| Elevar, Abaixar | Sobe a altura ou a escava enquanto você segura o botão. Um clique faz um calombo pequeno |
| Suavizar | Relaxa cada vértice em direção aos vizinhos, para amaciar calombos e cristas agudas |
| Nivelar | Puxa o chão para a altura em que o traço começou, para construir plataformas, estradas e platôs |
| Ruído | Acrescenta pequenos calombos aleatórios, para o chão plano parecer menos artificial |
| Terraço | Encaixa as alturas em degraus de **passo** unidades (32 por padrão), para arrozais ou penhascos em degraus. O campo aparece ao lado do modo |
| Pintar alfa, Apagar alfa, Pintar camada | Pintam em vez de dar forma. Os modos alfa são pensados para displacements, Pintar camada pinta a camada do terreno que você define no campo **camada** ao lado do modo. Em um terreno a [ferramenta Mistura](terrain-painting.md) faz isso com mais controle |
| Buraco, Tapar buraco | Corta células inteiras ou as preenche de novo, colisão incluída. Use para entradas de caverna ou de bunker, veja [Cavernas e saliências](caves-overhangs.md) |

Ctrl+roda redimensiona o brush, Shift troca elevar e abaixar, e segurar Ctrl suaviza seja qual for o modo.

O campo **força** na barra de opções define a rapidez com que o brush trabalha. Elevar e Abaixar movem o centro do brush 8 unidades por segundo por ponto de força, seja qual for o **raio**, então o padrão 4 constrói 32 unidades por segundo. Para colinas e ilhas aumente a força em vez de segurar por um minuto.

Mantenha o raio acima do tamanho da célula do terreno, um brush menor só empurra o único vértice sob ele (a barra de status avisa quando isso acontece).

Para recomeçar, *Terreno > Esculpir > Nivelar*, ou **Nivelar** no Inspetor, volta o terreno selecionado inteiro à altura zero.

> [!TIP]
> Dê forma com um brush grande e suavize depois de cada passada grosseira. Brushes pequenos deixam calombos.

## Simetria { #symmetry }

Para esculpir os dois lados de um vale ou de uma ilha de uma vez, ligue a simetria. As ferramentas **Esculpir** e **Mistura** têm o controle na barra de opções, logo depois de **força** e dos campos do modo.

![O menu de simetria aberto na barra de opções da ferramenta Esculpir, listando desativada, X, Z e X e Z](../assets/terrain/symmetry-menu.png)

| Controle | O que faz |
| --- | --- |
| Menu **simetria desativada** | Escolha **X** para espelhar as metades oeste e leste uma na outra, **Z** para norte e sul, ou **X e Z** para as duas. O menu fechado passa a dizer **simetria X**, **simetria Z** ou **simetria X e Z** |
| Menu **centro do terreno** | Só aparece com a simetria ligada. Ele decide onde ficam os planos de espelho: **centro do terreno** (o meio de cada terreno), **origem do mundo** ou **personalizado** |
| Campos **x** e **z** | Só com **personalizado**, um por eixo espelhado, a posição do plano em unidades. Eles começam nos planos que você estava usando, então nada pula |

Com a simetria ligada, as vistas desenham os planos enquanto Esculpir ou Mistura é a ferramenta ativa, como um contorno vermelho para X e um azul para Z. Uma linha acompanha o chão onde cada plano o corta, e uma cópia mais fraca do anel do brush mostra onde os toques espelhados vão cair antes de você apertar.

![A vista 3D com simetria X e Z, mostrando os dois planos de espelho e o anel do brush repetido nos quatro quadrantes](../assets/terrain/symmetry-planes.png)

Arraste como de costume. Cada toque é aplicado no cursor e nos pontos espelhados dele, e o traço inteiro continua sendo um passo de desfazer. Um toque sobre um plano, ou um que se sobrepõe ao próprio espelho, conta uma vez, então o chão não sobe o dobro ali. Ruído, ruído de mistura e spray saem como uma imagem espelhada exata em vez de um padrão aleatório novo.

Com **centro do terreno** cada terreno espelha em torno do próprio meio, e os displacements espelham em torno do meio de todos juntos. Esculpir e Mistura compartilham a configuração. Cada aba de mapa guarda a sua, um mapa novo ou aberto começa com a que você estava usando mas volta de **personalizado** para **centro do terreno**, e ela começa desligada de novo toda vez que você abre o editor.

> [!TIP]
> Para espelhar entre vários terrenos lado a lado, ou em torno de um ponto que não é o meio deles, use **origem do mundo** ou **personalizado**. Ponha o plano sobre uma linha de vértices ou bem no meio entre duas para um encaixe exato, qualquer outra posição só espelha de forma aproximada.

<div class="td-mcp" markdown>

A ação `sculpt`, a ferramenta `blend` e `terrain_edit` (`sculpt`, `sculpt_path`, `paint_path` e `holes` com `center`) aceitam um argumento `symmetry`, `off`, `x`, `z`, `xz` ou um objeto `{axes, center, x, z}`. `set_editor` define o que a barra de ferramentas usa e `get_state` o informa, veja [MCP](../mcp.md).

</div>

## No Godot { #in-godot }

Todo terreno vira um nó `TrenchDotTerrain`, um `StaticBody3D` (o nó do Godot para chão sólido que nunca se move) cuja colisão corresponde ao que ele desenha. Para o código de gameplay, `height_at(x, z)` devolve a altura do chão numa posição local, útil para pôr coisas no chão durante a execução.

<div class="td-mcp" markdown>

## MCP { #mcp }

A op `sculpt` da ferramenta `terrain_edit` aplica uma única pincelada, e a força dela é o quanto essa pincelada move o centro, de novo seja qual for o raio. Segurar um traço por um segundo com força 4 equivale a uma pincelada de força 32.

A mesma mudança de tamanho é `terrain_edit` com `op: "resize"`, que recebe `north`, `south`, `west` e `east` em células, ou `cells: [x, z]`, e `cell_size`. Adicione `dry_run: true` para ler antes as células, os limites, `scatter_removed` e `detail_lost` do resultado. `fill` é `edge` ou `zero` e `scatter` é `drop` ou `keep`, como no Inspetor. Veja [MCP](../mcp.md) para os limites.

</div>
