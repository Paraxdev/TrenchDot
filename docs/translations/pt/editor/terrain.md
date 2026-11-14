---
source: caa4681b9da88e6c8635967bd981136c01a89951
---

# Terreno { #terrain }

Um terreno é uma grade grande de alturas que você esculpe como argila, com até quatro texturas pintadas por cima. Use-o para chão aberto, como colinas, praias e vales, onde moldar brushes à mão levaria uma eternidade.

Esta página cobre como criar um e dar forma a ele com a ferramenta Esculpir (G). Texturizá-lo está em [Camadas de terreno](terrain-layers.md) e [Pintura de terreno](terrain-painting.md).

## Criando um terreno { #creating-a-terrain }

1. Abra a janela

    Vá em *Terreno > Criar terreno…* na barra de menus.

    ![O menu Terreno aberto com Criar terreno… sob o mouse](../assets/terrain/menu-create-terrain.png)

2. Escolha um tamanho

    **Resolução** e **Tamanho da célula** decidem o tamanho que ele terá, e a linha **Tamanho** embaixo delas mostra o resultado em unidades e metros. O padrão de 129 x 129 vértices com células de 64 unidades dá 8192 unidades, 256 m. Veja [Escolhendo o tamanho da célula](#picking-the-cell-size) abaixo.

    ![A janela Criar terreno com os padrões: 129 x 129, células de 64 u, Colinas e os quatro campos de camada](../assets/terrain/create-terrain-dialog.png)

3. (Opcional) Escolha uma forma inicial

    **Forma** é o chão de onde você parte. O padrão é Colinas, as outras são Plano, Montanha, Ilha, Vale e Cristas. **Importar PNG…** lê um mapa de altura em tons de cinza no lugar, onde pixels mais claros são mais altos. As duas opções só funcionam aqui, na criação.

4. (Opcional) Escolha as camadas

    **Camada base**, **Camada de declive**, **Camada de pico** e **Camada baixa** são as quatro texturas do terreno, e **Usar atual** coloca o material em que você clicou por último. Com **Pintar camadas por declive e altura** marcado elas são pintadas na hora, veja [Pintar auto](terrain-painting.md#auto-paint).

5. Clique em **Criar** na parte de baixo

    O terreno fica centralizado no cursor 3D, ou no **Centro** que você digita com **Cursor 3D** desmarcado, e essa altura vira o zero do terreno. Você deve ver o novo terreno selecionado, com a ferramenta Esculpir já ativa.

## Escolhendo o tamanho da célula { #picking-the-cell-size }

| Configuração | Significado |
| --- | --- |
| Resolução | Quantos vértices correm ao longo de cada lado, de 33 a 1025. Mais vértices guardam mais detalhe, mas custam mais para desenhar e esculpir |
| Tamanho da célula | A distância em unidades de mapa entre dois vértices vizinhos |

257 vértices com células de 64 unidades, por exemplo, dão 16384 unidades, 512 m.

Nada fica entre dois vértices, então o tamanho da célula é o detalhe mais fino que o terreno consegue guardar. Uma cordilheira pede células grandes, um jardim pede pequenas.

Escolheu errado? Você pode [mudar o tamanho](#changing-the-size) a qualquer momento depois de criar o terreno.

**Células por chunk** (32 por padrão) divide o terreno em pedaços quadrados para o desenho e a colisão. Chunks menores se refazem mais rápido durante a escultura, maiores significam menos nós no Godot.

## Mudando o tamanho { #changing-the-size }

Um terreno não fica preso no tamanho com que você o criou. Você pode adicionar células em qualquer lado, cortá-las ou mudar o tamanho da célula, e o solo que você já esculpiu fica exatamente onde estava no mapa.

1. Selecione o terreno

    Clique nele em uma vista ou no Outliner. O bloco **Tamanho** dele fica no Inspetor, abaixo de **Origem**.

2. Digite a mudança

    **Células** mostra quantas células correm ao longo de X e Z. Digite um total ali e os dois lados opostos crescem ou encolhem por igual, com a célula ímpar indo para o lado leste ou sul. Os campos **Norte (-Z)**, **Sul (+Z)**, **Oeste (-X)** e **Leste (+X)** mudam um lado só. Um número positivo adiciona essa quantidade de células e um negativo as remove, então `+8` em Leste adiciona 8 células e `-2` em Sul corta 2.

    ![O bloco Tamanho com 8 células adicionadas a leste e 2 removidas ao sul, e a linha Depois mostrando o resultado](../assets/terrain/inspector-size.png)

    Nada muda ainda. Suas edições são um rascunho, desenhado como contorno nas vistas, e a linha **Depois** abaixo de **Aplicar** diz as células e o tamanho que você teria.

3. Escolha o que o solo novo recebe

    **Solo novo** aparece assim que um lado cresce. **Altura da borda** continua a borda mais próxima, tanto a altura quanto a pintura, então o terreno simplesmente fica mais longo. **Zero** dá às células novas altura 0 e a primeira camada, para uma faixa plana onde construir. A primeira célula nova ainda desce da borda antiga até 0, então a parte plana começa uma célula para fora.

4. Clique em **Aplicar**

    O terreno muda em um único passo, e Ctrl+Z o devolve exatamente. **Redefinir** descarta o rascunho em vez disso.

Tudo o que foi mantido aparece na mesma posição do mundo de antes: alturas, pintura e buracos das células que ficam. Se você adicionar células no lado oeste ou norte, a **Origem** do terreno se move para fora para acompanhar.

**Tamanho da célula** funciona de outro jeito. Ele mantém o tamanho do terreno no mapa na célula inteira mais próxima e refaz a grade com o novo espaçamento, então a contagem de células muda junto, e alturas e pintura são misturadas entre os vértices antigos. Quando o tamanho não é múltiplo da célula nova, o terreno cresce ou encolhe até meia célula, e **Células inteiras mudam o tamanho em** abaixo de **Depois** diz quanto. Voltar ao tamanho de célula anterior não desfaz esse arredondamento, Ctrl+Z desfaz. Células maiores jogam detalhe fora, e o Inspetor avisa com **Detalhes finos se perdem** antes de você aplicar. Células menores mantêm a forma, mas não conseguem adicionar detalhe que nunca existiu. Os lados se aplicam primeiro, depois o tamanho da célula.

> [!NOTE]
> Um terreno tem entre 1 e 2048 células ao longo de cada eixo, e um tamanho de célula entre 0.001 e 100000 unidades. Uma mudança que saia disso, ou que cortaria todas as células, é recusada com o motivo em vermelho, e **Aplicar** fica cinza.

### Scatter no terreno { #scatter-on-the-terrain }

O [scatter](scatter.md) pintado no terreno fica no chão. Instâncias sobre solo que continua lá sobem ou descem com a nova altura, então nada flutua nem afunda. Instâncias sobre solo cortado são removidas por padrão, e quando existe alguma, uma caixa **Remover n instâncias de scatter do solo removido** aparece abaixo de **Aplicar**. Desmarque-a para mantê-las onde estão, flutuando sobre o nada, se você pretende movê-las por conta própria. Um conjunto de scatter travado ou oculto, ou que está numa camada travada ou oculta, fica como está.

## Esculpindo { #sculpting }

Aperte G, escolha um modo na barra de opções da ferramenta no canto superior esquerdo e arraste sobre o terreno. Ela age nos terrenos selecionados, ou em todos eles quando nada está selecionado.

![O menu de modos da ferramenta Esculpir aberto na barra de opções da ferramenta, ao lado dos campos de raio e força](../assets/terrain/sculpt-modes.png)

Mantenha o botão do mouse apertado e o chão continua subindo sob o brush.

![Segurando Elevar com um brush de 900 unidades, uma colina se forma na ilha](../assets/terrain/sculpt-raise.gif)

| Modo | O que faz |
| --- | --- |
| Elevar, Abaixar | Sobe a altura ou a escava enquanto você segura o botão. Um clique faz um calombo pequeno |
| Suavizar | Relaxa cada vértice em direção aos vizinhos, para amaciar calombos e cristas agudas |
| Nivelar | Puxa o chão para a altura em que o traço começou, para construir plataformas, estradas e platôs |
| Ruído | Acrescenta pequenos calombos aleatórios, para o chão plano parecer menos artificial |
| Terraço | Prende as alturas em degraus de **passo** unidades, para arrozais ou penhascos em degraus. O campo aparece ao lado do modo |
| Pintar alfa, Apagar alfa, Pintar camada | Pintam em vez de moldar. Os modos alfa são pensados para displacements, Pintar camada pinta a camada de terreno definida ao lado do modo. Num terreno, a [ferramenta Mistura](terrain-painting.md) faz isso com mais controle |
| Buraco, Tapar buraco | Recorta células inteiras ou as preenche de volta, colisão incluída. Use para entradas de caverna ou de bunker |

Ctrl+roda redimensiona o brush, Shift troca elevar e abaixar, e segurar Ctrl suaviza seja qual for o modo.

**força** define a rapidez com que o brush trabalha. Elevar e Abaixar movem o centro do brush 8 unidades por segundo por ponto de força, seja qual for o raio, então o padrão 4 constrói 32 unidades por segundo. Para colinas e ilhas aumente a força em vez de segurar por um minuto.

Mantenha o raio acima do tamanho da célula do terreno, um brush menor só empurra o único vértice sob ele (a barra de status avisa quando isso acontece).

Para recomeçar, *Terreno > Esculpir > Nivelar*, ou **Nivelar** no Inspetor, volta o terreno selecionado inteiro à altura zero.

> [!TIP]
> Dê forma com um brush grande e suavize depois de cada passada grosseira. Brushes pequenos deixam calombos.

## No Godot { #in-godot }

Todo terreno vira um nó `TrenchDotTerrain`, um `StaticBody3D` (o nó do Godot para chão sólido que nunca se move) cuja colisão corresponde ao que ele desenha. Para o código de gameplay, `height_at(x, z)` devolve a altura do chão numa posição local, útil para pôr coisas no chão durante a execução.

<div class="td-mcp" markdown>

## MCP { #mcp }

A op `sculpt` da ferramenta `terrain_edit` aplica uma única pincelada, e a força dela é o quanto essa pincelada move o centro, de novo seja qual for o raio. Segurar um traço por um segundo com força 4 equivale a uma pincelada de força 32.

A mesma mudança de tamanho é `terrain_edit` com `op: "resize"`, que recebe `north`, `south`, `west` e `east` em células, ou `cells: [x, z]`, e `cell_size`. Adicione `dry_run: true` para ler antes as células, os limites, `scatter_removed` e `detail_lost` do resultado. `fill` é `edge` ou `zero` e `scatter` é `drop` ou `keep`, como no Inspetor. Veja [MCP](../mcp.md) para os limites.

</div>
