# Campañas de generación para el runner

**Estado:** implementado para `STD-TESTING-SHRINK-001`

Este contrato describe la vista de tooling que conecta los helpers puros de
`std.testing` con el `RuntimeRunner` existente. No añade una keyword, no crea
un registro runtime y no modifica el árbol estático descubierto por
`tondo test`.

## Una única ruta de ejecución

Una campaña se materializa con `GenerationPlan` y `GeneratedCases<T>` desde
`tondo_compiler::test_generation`:

~~~rust
let plan = GenerationPlan::new("parser-property", 0x5eed, 100)?;
let cases = GeneratedCases::collect(&plan, |generator| {
    generator.next_int(-100, 100)
})?;
let result = cases.run_with_shrink(&runtime, |context, value| {
    check_property(context, value)
})?;
~~~

La closure de generación recibe un `Generator::for_case(seed, case_index)`
nuevo para cada caso. Por tanto, el valor no depende de cuántos casos se hayan
ejecutado antes y `GeneratedCases::replay` reconstruye el mismo input con el
mismo par `GenerationId`.

Los casos se pasan a `RuntimeRunner::run` como programas efímeros de tooling.
Cada ejecución obtiene el bootstrap, envelope, heap, executor y cleanup de un
worker nuevo. La respuesta se reordena por `case_index`, nunca por completion
order ni por la planificación de threads. Los IDs sintéticos usan un ancho
decimal fijo únicamente para la frontera interna; no se publican como
`TestEntry`, suite ni test dinámico.

## Shrinking determinista

`run_with_shrink` busca el primer caso fallido en orden de generación y llama al
`std.testing::shrink` público en cada nivel. Consume candidatos en el orden que
entrega el protocolo `Shrink` sellado, conserva la primera mejora que sigue
fallando y repite hasta que no haya mejora o se alcance `shrink_depth`. Cada
candidato vuelve a pasar por `RuntimeRunner`, de modo que no comparte estado,
recursos, snapshots, tags, logs ni memoria con el caso anterior.

La operación no captura pánicos dentro del helper `shrink`; si la propiedad
pánica al ejecutarse, el runtime la reporta como fallo y el candidato puede
continuar el proceso de minimización. No se ejecutan predicates desde
`shrink` mismo. El resultado conserva el caso original, el valor minimizado,
la profundidad y el número de evaluaciones para que el caller decida cómo
adjuntar o presentar la evidencia.

## Límites y compatibilidad

`GenerationLimits` valida antes de reservar o ejecutar: máximo de 100.000
casos, 4.096 candidatos por nivel y 64 niveles. `std.testing::shrink` aplica
el mismo máximo de candidatos y solo acepta las formas intrínsecas del
protocolo sellado; un tipo de usuario no puede instalar una implementación
alternativa. Un límite inválido no materializa un prefijo parcial.

La campaña no cambia `tondo-test-report-0.1/7`, JUnit, snapshots, tags,
retries ni repeat. Es una API de tooling sobre el runner; un proyecto Tondo
continúa escribiendo tests y usando `Generator` explícitamente dentro de una
hoja ordinaria.
