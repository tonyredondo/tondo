# Contrato de frontend de colecciones compartidas

Estado: `verified` para `STD-SYNC-COLLECTION-FRONTEND-001` en Tondo 0.1.
Este documento cierra la sintaxis, el CST, la resolución nominal y el HIR de
los cinco literales cualificados de `std.sync`; no afirma que exista todavía un
runtime ejecutable. El registro machine-readable es
[`testing/stdlib-sync-collection-frontend.json`](../../testing/stdlib-sync-collection-frontend.json)
y la implementación posterior está separada en
`STD-SYNC-COLLECTION-IMPL-001`.

## Superficie cerrada

Los únicos nombres que habilitan el azúcar son las identidades externas del
módulo bootstrap `toolchain:std:0.1-bootstrap::sync`:

```tondo
sync.Array[T]
sync.Map[K, V]
sync.Set[T]
sync.Stack[T]
sync.Queue[T]
```

El path debe estar cualificado. Un alias de import conserva la identidad, por
lo que `import std.sync as concurrent` permite `concurrent.Array[...]` y
`concurrent.Map[:]`; un tipo o módulo del usuario con el mismo spelling no
puede activar el literal. No existen keywords nuevas ni aliases globales
`SArray`, `SMap` o `SSet`.

La posición decide la lectura del corchete sin inspeccionar valores de runtime:

- En posición de tipo, `sync.Array[Int]` es una aplicación genérica ordinaria.
- En posición de expresión, `sync.Array[1, 2]` es un literal concurrente.
- `sync.Map[:]` es el único literal map vacío.
- Array, set, stack y queue vacíos necesitan un tipo esperado, por ejemplo
  `let values: sync.Array[Int] = sync.Array[]`.

Las secuencias admiten elementos separados por coma y trailing comma. Map usa
pares `key:value`, admite una o varias entradas y también trailing comma. Todos
los operandos se comprueban y evalúan de izquierda a derecha. Las reglas de
duplicados constantes son las mismas que para los literales ordinarios: `E1116`
para claves de map y `W1011` para valores de set. Un vacío sin contexto produce
`E1101`; una forma de corchetes inválida o un slice en una secuencia produce
`E1102`.

## CST, formatter y resolución

El lexer no reserva ninguna palabra. El parser conserva la forma lossless
`PathExpr + BracketPostfix`, incluyendo corchetes vacíos, `[:]`, comentarios,
trivia y trailing comma. El formatter puede volver a imprimir el árbol sin
perder información y una segunda pasada produce los mismos bytes.

Un corchete con un solo `:` sigue siendo ambiguo con un slice ordinario hasta
la resolución semántica. Cuando se observan dos o más pares en el mismo nivel,
el parser conserva todos los elementos como `BracketItem`; la identidad externa
es la que decide si se reclasifica como map. Así `values[start:end]` mantiene
su significado existente y ningún path de usuario obtiene el azúcar por forma.

El checker busca la referencia del último segmento y exige namespace `Type`,
package, módulo y declaración exactos. El tipo resultante es nominal e
invariante con la aridad correspondiente: un parámetro para Array/Set/Stack/
Queue y dos para Map. Un tipo esperado puede aportar esos argumentos para
resolver un literal vacío; una unión solo se acepta cuando contiene una única
instancia compatible. Los errores recuperan con una expresión tipada y no
publican una construcción parcial.

El HIR representa la construcción mediante el marcador interno
`std.sync.collectionLiteral`, con el nominal y los operandos ya comprobados.
El verifier vuelve a comprobar identidad, aridad y límites antes de cualquier
backend. El lowering MIR reconoce el marcador como frontera explícita y falla
con `STD-SYNC-COLLECTION-IMPL-001` hasta que exista la implementación de handles,
reclamación y operaciones concurrentes. No se añade una API pública ni se
simula un runtime.

## Evidencia ejecutable

La cobertura del slice está repartida entre:

- parser: losslessness, aliases, los cinco forms, map de una y varias entradas,
  vacío y round-trip;
- HIR: identidad nominal, inferencia contextual, errores de contexto/forma,
  duplicados constantes y recovery;
- MIR: frontera negativa que impide atravesar el marcador antes del bloque de
  implementación.

Los tests canónicos están enumerados en el registro JSON y se ejecutan también
por `scripts/stdlib-sync-collection-frontend-check.sh` y
`scripts/stdlib-sync-collection-frontend-test.sh`. La evidencia no convierte
esta leaf de frontend en conformidad de runtime: `STD-SYNC-COLLECTION-IMPL-001`,
`STD-SYNC-COLLECTION-ITER-001` y sus campañas de modelo, rendimiento,
conformance y documentación siguen siendo hojas posteriores.
