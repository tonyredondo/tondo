# Tondo LLM Form

**Estado:** borrador normativo en desarrollo; todavía no implementado

**Identidad actual:** `tondo-llm-form-draft`

**Extensión convencional:** `.tlf`

**Lenguaje base:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md)

**Toolchain:** [Tondo Toolchain 0.1](./TONDO_TOOLCHAIN_SPEC.md)

Tondo LLM Form, abreviado **TLF**, es el dialecto de intercambio textual de
Tondo para agentes y modelos de lenguaje. Su objetivo es aumentar la cantidad
de código Tondo correcto que puede producirse por token sin crear una segunda
semántica, una segunda librería ni un segundo compilador.

Un documento TLF nunca se compila directamente. Primero se expande de forma
determinista a fuente Tondo ordinaria y canónica; a partir de esa frontera se
utilizan exactamente el lexer, parser, resolución, type checker, lowering y
backends normales.

Las palabras **debe**, **no debe**, **puede**, **error** y **canónico** son
normativas en este documento.

---

## 1. Motivación y criterio de éxito

TLF no optimiza caracteres aislados. Su métrica primaria es:

> programas Tondo correctos y aceptados por token total producido.

El coste total incluye la salida inicial, los diagnósticos necesarios y todas
las reparaciones hasta alcanzar un programa aceptado. Una representación más
corta que incremente suficientemente la tasa de errores, la ambigüedad o las
repeticiones no constituye una mejora.

La evaluación separa al menos:

- tokens de salida;
- bytes de salida;
- validez léxica y sintáctica en el primer intento;
- aceptación del type checker;
- equivalencia observable con la intención del caso;
- rondas y tokens de reparación;
- precisión de spans y patches;
- tiempo y memoria de expansión.

Ninguna cifra medida con un único tokenizer, modelo o corpus puede convertir
una abreviatura en parte del formato. La adopción exige una mejora robusta en
una matriz representativa.

## 2. Principios

### 2.1 Una sola semántica

TLF no añade tipos, conversiones, imports, defaults, inferencia, efectos,
operadores, reglas de ownership ni comportamiento runtime. Toda construcción
expande a tokens que ya existen en Tondo.

### 2.2 Reconocible antes que críptico

Las palabras clave, identificadores, literales, delimitadores y operadores de
Tondo conservan su spelling. El ahorro del draft proviene de eliminar trivia y
layout redundantes, no de sustituir `fn`, `let`, `return` o tipos comunes por
códigos opacos.

### 2.3 Público y negociado

TLF es un formato documentado. No depende de prompts secretos ni de sintaxis
oculta por proveedor. Su identidad y source form se seleccionan fuera de los
bytes del documento; no se gasta un prólogo repetido dentro de cada respuesta.

### 2.4 Canónico

Existe una única codificación canónica para la vista semántica de una fuente.
Un decoder puede aceptar trivia o separadores redundantes para producir mejores
diagnósticos, pero cualquier hash, cache o comparación usa primero la forma
canónica.

### 2.5 Independiente del tokenizer

El draft no contiene una variante OpenAI, Anthropic, Mistral, Qwen ni de otro
proveedor. Un único documento debe ser útil con familias BPE y SentencePiece
diferentes. La matriz concreta de evaluación puede evolucionar mientras Tondo
no haya sido publicado, pero la salida no cambia según el modelo que la genera.

### 2.6 Acotado y no ejecutable

Codificar y expandir son transformaciones puras, deterministas, lineales en el
tamaño de entrada y sometidas a límites explícitos. TLF no ejecuta macros,
generators, filesystem, red, procesos, environment, reloj ni código Tondo.

## 3. No objetivos

TLF no pretende:

- reemplazar `.to` como formato mantenido por humanos o almacenado en Git;
- ser una serialización del AST, HIR o MIR;
- preservar byte a byte formato, líneas vacías o comentarios ordinarios;
- comprimir binariamente programas;
- definir un protocolo de patches estructurales;
- abreviar identificadores mediante índices de scope;
- entrenar o distribuir un tokenizer propio;
- hacer válido un programa Tondo inválido;
- ocultar información semántica que el decoder tenga que inferir;
- permitir que el compilador acepte otra semántica bajo una extensión distinta.

La edición eficiente de código existente mediante patches es una capa futura e
independiente. No forma parte de `tondo-llm-form-draft`.

## 4. Modelo y términos

Se utilizan las funciones conceptuales siguientes:

- `F(s)`: formatter canónico Tondo aplicado a una fuente válida `s`.
- `S(s)`: vista de `s` que elimina comentarios ordinarios y secciones vacías,
  conserva shebang y comentarios de documentación, y mantiene los mismos tokens
  significativos.
- `P(s)`: codificación canónica de `F(S(s))` como TLF.
- `E(t)`: expansión válida de un documento TLF `t` a Tondo canónico.
- `C(t)`: canonicalización TLF, definida como `P(E(t))`.

Un **token significativo** es un token físico Tondo que no es whitespace,
newline físico ni comentario ordinario. Los comentarios de documentación y el
shebang se conservan como átomos especiales porque forman parte del contrato de
documentación o ejecución de una fuente.

Un **NL lógico** es el token `NL` producido por el algoritmo normativo de Tondo
5.2 y consumido por su gramática. No equivale a todo `LF` físico.

Un **separador TLF** es `;` fuera de un literal, comentario o shebang. Expande a
un `NL` lógico.

El **source form** es uno de `module`, `script` o `fragment`. Forma parte del
request de codificación o expansión; no se adivina a partir del contenido.

## 5. Invariantes

Para toda fuente Tondo válida `s`:

~~~text
E(P(s)) == F(S(s))
P(E(P(s))) == P(s)
~~~

La igualdad es byte a byte. Además:

~~~text
compile(E(P(s))) ≡ compile(s)
~~~

`≡` exige la misma semántica Tondo: tipos, diagnósticos semánticos, orden de
evaluación, ownership, errores, pánicos, efectos, outputs y comportamiento
runtime observables. La posición física de tokens y la ausencia de comentarios
ordinarios no son observables del programa.

Para todo documento TLF aceptado `t`:

~~~text
P(E(t)) == C(t)
C(C(t)) == C(t)
~~~

La conversión no promete `E(P(s)) == s`: el formatter, la normalización NFC y
la eliminación de comentarios ordinarios impiden un round-trip byte a byte.
Esta pérdida es deliberada y queda fuera de la semántica compilada.

## 6. Identidad, extensión y selección

La única identidad vigente durante el desarrollo es:

~~~text
tondo-llm-form-draft
~~~

No existe una cadena `v1`, `v2` ni una edición histórica publicada. Mientras
Tondo continúe sin release, un cambio incompatible actualiza este mismo draft,
sus vectores y su implementación conjuntamente.

La extensión convencional es `.tlf`, pero la extensión por sí sola no concede
autoridad para compilar ni sustituye la selección explícita del formato. Una
API embebida negocia la identidad exacta; la CLI `tondo llm` la conoce por el
toolchain actual.

No hay prólogo dentro del documento. Esta decisión evita pagar tokens fijos en
cada fragmento y evita que dos fuentes con el mismo programa difieran solo por
un header.

Un `.to` nunca activa TLF mediante sniffing. Un `.tlf` nunca entra directamente
en `tondo fmt`, `tondo check`, `tondo run`, `tondo test`, el resolver de módulos
o un generator. Debe pasar por la frontera explícita de expansión.

## 7. Codificación de caracteres y source form

TLF utiliza UTF-8. Un byte sequence inválido se rechaza antes de producir salida.
Identificadores siguen las reglas Unicode y NFC de Tondo; literales conservan
su spelling exacto después del formatter base.

El request declara exactamente uno de:

- `module`: fuente importable sin shebang ni sentencias top-level;
- `script`: raíz ejecutable, con shebang opcional y sentencias top-level;
- `fragment`: fragmento de tooling bajo el contexto definido por el caller.

El encoder y decoder usan el mismo source form. Cambiarlo cambia el request y
obliga a validar de nuevo; nunca se recupera mediante heurística.

## 8. Superficie léxica

TLF reutiliza todos los tokens físicos de Tondo y añade un único token exterior:

~~~text
TLF_SEPARATOR = ";"
~~~

Fuera de strings, chars, raw strings, interpolaciones, comentarios y shebang:

- `;` representa una frontera lógica;
- space, tab, `LF` y `CRLF` son trivia TLF y no terminan una sentencia, salvo
  el final físico que cierra shebang o comentario de línea/documentación;
- un `CR` aislado es inválido;
- las reglas de maximal munch de Tondo siguen aplicándose;
- no existen escapes para keywords ni aliases de tokens.

Un `;` dentro de un literal o comentario es contenido ordinario y no se
interpreta como separador. Los finales físicos dentro de un literal multilínea
conservan exactamente la semántica Tondo.

### 8.1 Comentarios

El decoder acepta comentarios Tondo para facilitar outputs explicativos:

- un comentario de línea termina en el siguiente final físico;
- un comentario de bloque conserva sus reglas normales;
- un comentario de documentación debe permanecer unido a la declaración que
  documenta;
- el final físico que termina un comentario de línea se conserva al expandir y
  puede producir el `NL` lógico ordinario de Tondo. Esta forma se acepta para
  outputs explicativos, pero no aparece en TLF canónico salvo documentación.

`E(t)` conserva los comentarios escritos en `t` y el formatter los coloca de
forma canónica. `P(s)` conserva comentarios de documentación y elimina los
ordinarios. Por ello `C(t)` también elimina comentarios ordinarios.

### 8.2 Shebang

El shebang solo es válido en source form `script`, comienza en el byte cero y
termina en el primer final físico. El encoder lo conserva. Su `LF` terminal es
parte de la representación del shebang, no un separador `;` general.

## 9. Gramática exterior

La gramática exterior no duplica la gramática Tondo:

~~~ebnf
tlf_document    = [ shebang, physical_newline ],
                  { tlf_trivia | tondo_token | tlf_separator }, EOF ;

tlf_separator   = ";" ;

tlf_trivia      = horizontal_space
                | physical_newline
                | ordinary_comment
                | documentation_comment ;

horizontal_space
                = " " | "\t" ;

physical_newline
                = "\n" | "\r\n" ;
~~~

`tondo_token` significa cualquier token físico admitido por el lexer Tondo en
el mismo estado léxico. La validez estructural se define por expansión: al
reemplazar cada `tlf_separator` por un `NL` lógico, normalizar trivia y añadir el
final requerido, el resultado debe ser una fuente válida para el source form
declarado.

Esta definición mantiene una sola gramática semántica. TLF solo transporta la
secuencia de tokens que esa gramática recibe.

## 10. Algoritmo canónico de codificación

`P(s)` se calcula en este orden:

1. Validar `s` con el lexer y parser Tondo del source form declarado. Ante
   cualquier diagnóstico no se produce TLF parcial.
2. Eliminar comentarios ordinarios y secciones vacías mediante el CST sin
   pérdida, reinsertando el separador mínimo necesario para no unir tokens.
3. Ejecutar `tondo fmt` sobre la fuente resultante. Esta salida es `F(S(s))`.
4. Lexear de nuevo esa salida y conservar tokens físicos, comentarios de
   documentación, shebang y tokens `NL` lógicos.
5. Eliminar whitespace y finales físicos exteriores salvo los que pertenecen a
   shebang, comentarios o literales.
6. Omitir todo `NL` lógico inmediatamente posterior a `{`. La gramática Tondo
   admite cero `NL` al comienzo de cada body; esta omisión no cambia su CST
   normalizado.
7. Convertir cada `NL` lógico restante en `;`.
8. Omitir el separador terminal: el decoder siempre añade exactamente un `LF`
   al final de la fuente expandida antes de formatear.
9. Entre dos tokens físicos consecutivos emitir la cadena vacía si su
   concatenación conserva exactamente los mismos dos tokens bajo el mismo
   estado léxico. En otro caso emitir un único space ASCII.
10. No emitir whitespace final ni `LF` terminal.

El paso 9 se define por identidad de kind, spelling y límites de los tokens, no
por una tabla aproximada de caracteres. Una implementación puede usar una tabla
precalculada si demuestra equivalencia byte a byte con esta definición.

No se omite un `NL` anterior a `}` de forma general. En blocks puede cerrar una
sentencia, en traits e impls termina un método y en `match` termina un arm. Una
versión futura solo podrá eliminar casos adicionales después de definirlos por
CST y probarlos en toda la gramática.

## 11. Algoritmo de expansión

`E(t)` se calcula en este orden:

1. Validar UTF-8, source form, identidad del formato y límites antes de reservar
   el output máximo.
2. Lexear TLF conservando estados de strings, chars, interpolaciones,
   comentarios y shebang. Los finales físicos exteriores son trivia, no `NL`.
3. Copiar cada token Tondo con su spelling exacto.
4. Sustituir cada `;` exterior por un `LF` físico y exigir que el lexer Tondo lo
   clasifique como exactamente un `NL` lógico en esa posición. Un separador en
   una continuación donde Tondo suprimiría el newline es TLF no canónico y se
   rechaza si no conserva la secuencia lógica esperada.
5. Insertar un único space cuando dos tokens copiados se unirían como otro token
   Tondo; no modificar contenido de literales o comentarios.
6. Conservar el final físico requerido por shebang y comentarios de línea; ese
   final se entrega al lexer Tondo y puede formar el `NL` asociado al comentario.
7. Añadir exactamente un `LF` al final.
8. Lexear y parsear el resultado como Tondo ordinario en el source form
   declarado.
9. Si es válido, ejecutar `tondo fmt` y publicar atómicamente sus bytes. Si no,
   no publicar fuente parcial.

El decoder no consulta símbolos, tipos, imports ni filesystem. Un documento que
requiera esa información para ser entendido no es TLF válido.

## 12. Canonicalización

`tondo llm fmt` calcula `C(t) = P(E(t))`. En particular:

- elimina whitespace prescindible;
- elimina comentarios ordinarios;
- normaliza comentarios de documentación mediante el formatter Tondo;
- elimina `;` inmediatamente posterior a `{`;
- elimina el `;` terminal;
- conserva un solo spelling NFC para identificadores;
- conserva el spelling normativo del formatter para `Option`, `Result` y demás
  formas normalizadas;
- no introduce aliases ni diccionarios.

Dos documentos TLF son semánticamente equivalentes cuando sus expansiones
canónicas son byte a byte iguales. Solo `C(t)` se utiliza para hashes o caches.

## 13. Source maps

Toda expansión produce conjuntamente:

- bytes TLF originales;
- bytes Tondo previos al formatter;
- bytes Tondo canónicos finales;
- mapa TLF → preformateado;
- mapa de edits del formatter;
- composición TLF → Tondo canónico.

Los offsets de las tres vistas son bytes UTF-8 y los ranges son semiabiertos.
La composición sigue la misma disciplina que los source maps de generación
estática:

- un token copiado se asocia a su range TLF exacto;
- el `LF` generado por `;` se asocia al byte del separador;
- spaces insertados tienen asociación de anchura cero con la frontera siguiente;
- layout introducido por el formatter se asocia mediante su edit map;
- un range inválido, invertido o solapado se rechaza; nunca se atribuye al
  archivo completo.

Una ubicación pública identifica además:

~~~text
representation = "tlf" | "tondo-expanded"
~~~

La ubicación primaria que recibe un agente está en TLF. La ubicación expandida
puede incluirse como relacionada para depuración del codec.

## 14. Diagnósticos y patches

Los errores semánticos y de sintaxis Tondo conservan su código normativo. Sus
spans se proyectan a TLF mediante el source map; el codec no los reemplaza por
un error genérico.

Los fallos propios de la frontera TLF se dividen en:

- identidad o source form ausentes/incompatibles;
- UTF-8 o token TLF inválido;
- separador ausente o redundancia que no puede expandirse a un CST válido;
- límite de input, tokens, nesting, output o segmentos agotado;
- source map imposible de componer.

La implementación reservará códigos estables dentro del bloque `E22xx` antes de
exponer la CLI. El contrato de cada código, precedencia y span se cerrará junto
con `TLF-DIAG-001`; este draft no inventa números sin implementación y tests.

Un patch aplicable a TLF:

- usa offsets de los bytes TLF entregados;
- no edita los bytes Tondo expandidos directamente;
- conserva strings y comentarios como átomos;
- se vuelve a validar y canonicalizar después de aplicarse;
- nunca depende del path físico ni de un handle del compilador.

## 15. CLI y APIs

La superficie prevista es:

~~~text
tondo llm encode [--source-form <module|script|fragment>] <source.to>
tondo llm decode [--source-form <module|script|fragment>] <source.tlf>
tondo llm check  [--source-form <module|script|fragment>] <source.tlf>
tondo llm fmt    [--source-form <module|script|fragment>] <source.tlf>
~~~

El default de CLI es `module`. `script` y `fragment` se seleccionan de forma
explícita. Una API embebida no tiene default: el request siempre incluye el
source form y la identidad exacta `tondo-llm-form-draft`.

Contratos de streams:

- `encode`, `decode` y `fmt` escriben únicamente el producto en stdout;
- `check` es silencioso cuando tiene éxito;
- diagnostics humanos van a stderr;
- diagnostics JSON usan el formato público normal y ranges TLF;
- un fallo no escribe producto parcial;
- los cuatro comandos son deterministas y no modifican el archivo de entrada.

`tondo llm check` ejecuta `decode` en memoria y después la ruta ordinaria de
`check`. No existe un type checker TLF.

## 16. Límites y complejidad

El request fija como mínimo:

- `max_input_bytes`;
- `max_tokens`;
- `max_nesting_depth`;
- `max_output_bytes`;
- `max_source_map_segments`;
- `max_diagnostics`.

Ningún límite se obtiene de environment o configuración global oculta. La
expansión tiene coste `O(input_bytes + output_bytes)` y utiliza una pila
explícita acotada para estados anidados. No utiliza la pila del host para
profundidad controlada por input.

El decoder comprueba el presupuesto antes de publicar cada crecimiento. Al
agotarlo descarta output, source map y diagnostics parciales salvo el único
diagnóstico terminal de límite.

Como TLF no posee aliases, macros ni referencias, un token de entrada no puede
expandir a una secuencia arbitrariamente grande. El formatter posterior sigue
sus propios límites normativos.

## 17. Ejemplos

Fuente Tondo:

~~~tondo
import std.console

type User = {
    name: String
    age: Int
}

fn greet(user: User): String {
    "Hello, {user.name}"
}

fn main() {
    let user = User { name: "Tony", age: 42 }
    console.println(greet(user))
}
~~~

TLF canónico:

~~~text
import std.console;type User={name:String;age:Int;};fn greet(user:User):String{"Hello, {user.name}";};fn main(){let user=User{name:"Tony",age:42};console.println(greet(user));}
~~~

La expansión vuelve a producir la fuente Tondo canónica anterior. Los `;`
posteriores a fields, statements y blocks aportan los `NL` exigidos por la
gramática. No aparece `;` inmediatamente después de `{` ni al final del archivo.

Un layout TLF no canónico pero aceptable puede usar líneas físicas:

~~~text
import std.console;
type User={
  name:String;
  age:Int;
};
~~~

Esos saltos físicos son trivia. `tondo llm fmt` devuelve la única línea canónica.

Los separadores dentro de strings no cambian:

~~~text
fn main(){let text="a;b";console.println(text);}
~~~

## 18. Conformidad

La suite TLF debe incluir, como mínimo:

### 18.1 Golden vectors

- módulos, scripts y fragments;
- shebang;
- imports y declaraciones públicas/privadas;
- records, enums, traits, impls, derives, functions y methods;
- todos los statements, expressions, patterns y types;
- strings normales, raw, multiline e interpolados con `;`;
- comentarios de línea, bloque y documentación;
- Unicode, NFC, chars y escapes;
- testing, async, process y unsafe;
- casos en los límites de nesting y tamaño.

Cada vector contiene fuente Tondo, TLF canónico, expansión Tondo, source map y
diagnósticos esperados.

### 18.2 Properties

Para un generador de CST Tondo válido:

~~~text
E(P(s)) == F(S(s))
P(E(P(s))) == P(s)
C(C(t)) == C(t)
parse(E(t)) succeeds
~~~

La suite compara además resultados de `check`, MIR observable, ejecución y
diagnósticos entre fuente original y expandida.

### 18.3 Negativos y fuzzing

Se prueban UTF-8 inválido, tokens truncados, literals sin cerrar, comments
anidados inválidos, separadores dentro/fuera de interpolaciones, nesting
adversario, límites en cada byte, source maps solapados y ausencia de output
parcial. Encoder y decoder tienen fuzz targets y corpus persistente.

### 18.4 Evaluación LLM

Antes de declarar TLF estable se ejecuta una matriz versionada de generación y
reparación. Para cada modelo/tokenizer se publica:

- prompt exacto y coste de enseñar el formato;
- seed y parámetros;
- casos nuevos y modificaciones;
- tokens de primera salida y totales;
- parse/typecheck/acceptance en primer intento;
- rondas de reparación;
- comparación con Tondo canónico y Tondo minificado.

TLF no se promociona si el ahorro bruto queda anulado por más reparaciones o si
la mejora solo existe en un proveedor.

La medición léxica y la evaluación de modelos son productos reproducibles del
repositorio, no tablas mantenidas a mano. El harness fija:

- revisión exacta de cada tokenizer o hash de su artefacto;
- manifest content-addressed del corpus y regla de deduplicación;
- versión del lexer/codec y parámetros completos;
- candidatos comparados y algoritmo canónico de cada transformación;
- resultados machine-readable y el Markdown derivado de esos resultados.

Un cambio de corpus, tokenizer, candidato o algoritmo invalida los resultados
anteriores y exige regenerarlos. El informe humano nunca es el único artefacto
capaz de reproducir un porcentaje.

### 18.5 Bundle de conformidad L0

TLF no se añade al linaje de conformidad G5 del lenguaje base. G5 conserva sus
cuatro autoridades —lenguaje, testing, toolchain y stdlib— porque un programa
`.to` no depende de TLF. L0 produce un bundle content-addressed separado que
fija como mínimo:

- esta especificación y la identidad del formato;
- implementación de encoder, decoder, canonicalizer y source maps;
- schema y golden vectors de la CLI;
- corpus de properties, fuzzing y límites;
- harness, manifests y resultados de medición léxica y evaluación LLM; y
- hashes de toolchain y frontend Tondo contra los que se probó la equivalencia.

El candidato completo de Tondo fija por hash tanto el bundle G5 como el bundle
L0. Ninguna evidencia TLF cubre requisitos del lenguaje base y ningún resultado
G5 sustituye las properties o evaluaciones de TLF.

## 19. Decisiones deliberadas del draft

### 19.1 Sin aliases de keywords

`fn`, `let`, `return`, `import`, `String`, `Array` y formas similares ya suelen
ser uno o pocos tokens. Cambiarlas por letras reduce bytes pero no el coste de
forma robusta y elimina patrones conocidos por los modelos.

### 19.2 Sin diccionario de identificadores

Las tablas locales solo ayudaron a una minoría del corpus explorado y exigen que
el modelo mantenga índices correctos. El beneficio medido no compensa errores,
headers ni complejidad de source maps.

### 19.3 Sin JSON ni S-expressions

Una serialización de AST repite nombres de fields y node kinds. Una forma
prefix/S-expression añade delimitadores y se aleja de los patrones de código
Tondo. Ambas siguen siendo baselines de investigación, no formatos normativos.

### 19.4 Sin binario ni Base64

Un wire binario puede reducir bytes, pero no es una salida textual natural de
un LLM. Base64 aumenta el tamaño y destruye locality para diagnósticos y patches.

### 19.5 Sin formas por proveedor

Versiones específicas por tokenizer fragmentarían prompts, caches, tests y
tooling. TLF optimiza el resultado robusto de una matriz, no el máximo local de
un vocabulario.

### 19.6 Sin shorthands estructurales por ahora

Omitir `fn`/`type` o introducir bindings `:=`/`~=` produjo una mejora pequeña
frente a la cinta léxica, pero exigiría una segunda gramática exterior. Queda
fuera hasta que una evaluación de programas correctos por token demuestre una
ganancia material después de reparaciones.

## 20. Evolución antes del primer release

TLF continúa siendo un draft único. Cambiar el formato exige actualizar en el
mismo cambio:

- este documento;
- ADR y toolchain afectados;
- encoder, decoder y source maps;
- golden vectors, properties y fuzz corpus;
- estudio reproducible de tokenizers;
- benchmark de generación/reparación;
- tracker, matriz de conformidad L0 y bundle separado.

Después del primer release, cualquier cambio incompatible requerirá el mecanismo
de negociación que se defina entonces. Este documento no crea hoy checkpoints
históricos ni promete compatibilidad que Tondo todavía no publica.
