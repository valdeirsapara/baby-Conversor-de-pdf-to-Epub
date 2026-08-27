# pdf_to_epub

Conversor de **PDF para EPUB** escrito em Rust, com uma interface de terminal (TUI) para
acompanhar a conversão em tempo real. Funciona com qualquer PDF — não é uma ferramenta
feita para um livro específico.

O objetivo principal do projeto é resolver o problema mais comum de conversores PDF→EPUB
simples: **imagens que somem, saem de ordem ou vão parar no lugar errado**. Aqui, cada
imagem é reposicionada exatamente entre os mesmos parágrafos onde ela aparecia no PDF
original, e os capítulos são detectados automaticamente a partir do próprio texto.

## Instalação

### Via Homebrew (macOS e Linux)

```bash
brew install valdeirsapara/pdf-to-epub/pdf_to_epub
```

### A partir do código-fonte

Requer [Rust](https://www.rust-lang.org/tools/install) (edition 2024) e acesso à internet
na primeira compilação, para baixar as dependências.

```bash
git clone https://github.com/valdeirsapara/baby-Conversor-de-pdf-to-Epub.git
cd baby-Conversor-de-pdf-to-Epub
cargo build --release
```

O binário fica em `target/release/pdf_to_epub`.

### Binários prontos

Binários pré-compilados (Linux e macOS Apple Silicon; macOS Intel ainda não publicado)
ficam disponíveis em
[Releases](https://github.com/valdeirsapara/baby-Conversor-de-pdf-to-Epub/releases) a
cada versão publicada.

## Principais recursos

- **EPUB de verdade (reflowable)**: o texto flui normalmente e se adapta a qualquer
  tamanho de fonte/tela — não é uma cópia "engessada" das páginas do PDF.
- **Imagens no lugar certo**: cada imagem é extraída e inserida no HTML exatamente na
  posição de leitura em que aparecia no PDF, entre os mesmos parágrafos.
- **Capítulos automáticos**: detecta títulos de capítulo (ex.: "Capítulo 1", "Chapter 2")
  por padrão de texto e por salto de tamanho de fonte, gerando um sumário navegável real.
- **TUI com progresso ao vivo**: uma tela de terminal guia a conversão passo a passo,
  com log de cada etapa (extração de texto, extração de imagens, detecção de capítulos,
  geração do EPUB).
- **Preview em HTML**: ao final, é possível abrir uma prévia do resultado (texto +
  imagens) direto no navegador, antes de considerar o EPUB definitivo.
- **Metadados editáveis**: título/autor/idioma são detectados a partir do próprio PDF (ou
  do nome do arquivo) e podem ser corrigidos na tela de confirmação antes de gerar o EPUB.

## Como funciona

O `pdf-extract` (biblioteca Rust usada para ler texto de PDFs) tem um bug conhecido: se
uma página desenha uma imagem diretamente (comum em livros ilustrados), ele tenta
reinterpretar os bytes da imagem como se fossem instruções de desenho do PDF e **trava
(panic)**. Para evitar isso, antes de extrair o texto de cada página, o conversor:

1. Lê o *content stream* da página com [`lopdf`](https://crates.io/crates/lopdf).
2. Identifica todo comando que desenha uma imagem, guarda a posição exata onde ela
   aparece (via a matriz de transformação corrente) e **remove só esse comando**.
3. Passa o restante do conteúdo (já "higienizado") para o `pdf-extract`, que agora só
   processa texto de verdade e não corre risco de travar.
4. Depois, mescla o texto reconstruído com as imagens removidas, ordenando tudo pela
   posição vertical na página — é assim que cada imagem acaba exatamente entre os
   parágrafos certos no EPUB final.

As imagens em si são reaproveitadas como estão quando já são JPEG (a maioria dos casos),
e reconvertidas para PNG quando são dados de imagem "crus" (Flate/RGB/CMYK/paleta
indexada). Capítulos são detectados combinando padrões de texto comuns
("Capítulo N", "Chapter N", "Parte N"...) com um salto perceptível no tamanho da fonte em
relação ao corpo do texto — títulos quebrados em várias linhas são unidos automaticamente.

Esse processo é heurístico por natureza (não existe uma forma 100% determinística de
"adivinhar" onde um capítulo começa a partir de um PDF), então cada capítulo detectado é
registrado no log da TUI para conferência.

## Uso

### Modo interativo (TUI)

```bash
./target/release/pdf_to_epub
# ou já passando o caminho do PDF:
./target/release/pdf_to_epub "caminho/para/o/livro.pdf"
```

Fluxo das telas:

1. **Caminho do PDF** — digite ou cole o caminho do arquivo `.pdf`. `Enter` confirma.
2. **Confirmação de metadados** — título, autor e idioma detectados automaticamente
   (editáveis com `Tab` para trocar de campo). `Enter` inicia a conversão.
3. **Progresso** — barra de status e log em tempo real de cada etapa. Ao terminar:
   - `p` — abre uma prévia (`preview.html`) no navegador padrão
   - `o` — abre a pasta onde o EPUB foi salvo
   - `n` — volta para converter outro PDF
   - `q` — sai

O EPUB e o `preview.html` são salvos na mesma pasta do PDF de entrada.

> **Nota (WSL2)**: a ação de abrir o preview/pasta no navegador usa `wslpath` +
> `explorer.exe` para abrir no lado Windows. Fora do WSL, essa ação pode não funcionar,
> mas o resto do conversor funciona normalmente.

### Modo headless (sem interface, para scripts/depuração)

```bash
./target/release/pdf_to_epub --headless "caminho/para/o/livro.pdf" [pasta_de_saida]
```

Roda o pipeline completo imprimindo o log no terminal, sem abrir a TUI.

## Limitações conhecidas

- A separação em capítulos é heurística; PDFs sem título de capítulo destacado (fonte
  maior ou padrão "Capítulo N") podem não ser divididos corretamente.
- A ordenação de texto/imagens assume layout de coluna única por página; PDFs com
  múltiplas colunas podem misturar a ordem de leitura.
- Imagens com os codecs `JPXDecode` (JPEG2000), `CCITTFaxDecode` ou `JBIG2Decode` não são
  suportadas e são puladas (com aviso no log) em vez de travar a conversão.

## Estrutura do projeto

| Módulo | Responsabilidade |
|---|---|
| `pdf_parse.rs` | Higieniza e lê o conteúdo do PDF; reconstrói texto e posição das imagens |
| `images.rs` | Extrai e reconverte as imagens embutidas no PDF |
| `chapters.rs` | Detecta capítulos a partir do fluxo de texto |
| `epub_gen.rs` | Monta o HTML de cada capítulo e gera o arquivo `.epub` |
| `preview.rs` | Gera o `preview.html` autocontido e o abre no navegador |
| `tui.rs` | Interface de terminal (telas, progresso, ações) |
| `pdf_model.rs` | Tipos de dados compartilhados entre os módulos |
| `main.rs` | Ponto de entrada (modo TUI e modo `--headless`) |
