# Tondo Toolchain 0.1

**Estado:** contrato bootstrap implementado

**Versión:** `tondo-toolchain-0.1/1`

**Especificación de lenguaje:** [Tondo 0.1-draft.8](./TONDO_LANGUAGE_SPEC.md)

Esta especificación define la frontera de proyecto del toolchain Tondo 0.1:
manifiesto, lockfile, selección de fuentes, grafo de paquetes, interfaces
compiladas, metadatos de artefacto y unidades privilegiadas. No modifica la
sintaxis ni la semántica de un archivo `.to`.

Las palabras **debe**, **no debe**, **puede** y **error** son normativas para el
toolchain bootstrap cuando describen formatos o validaciones ya implementados.
La resolución de versiones, descarga de paquetes, ejecución de generadores y
una ABI nativa general permanecen fuera de este contrato.

## 1. Objetivos

La frontera de proyecto tiene cinco propiedades:

1. El compilador recibe un grafo cerrado; nunca descubre paquetes o archivos.
2. Target, perfil, capacidades, features y source sets son entradas explícitas.
3. Toda entrada física se declara y queda fijada por SHA-256 antes de analizar
   fuente.
4. Una interfaz incompatible se rechaza antes de lexear o typecheckear el
   consumidor.
5. Entradas idénticas producen bytes idénticos de interfaz y artefacto.

El compilador no consulta red, reloj, variables de entorno, directorio actual,
aleatoriedad ni procesos. La CLI puede leer los paths que el plan cerrado le
solicita, pero no puede añadir entradas por descubrimiento.

## 2. Identidades y codificación

### 2.1 JSON

Los cinco formatos de este documento utilizan UTF-8 JSON:

- manifiesto: `tondo-manifest-0.1/1`;
- lockfile: `tondo-lock-0.1/1`;
- interfaz: `tondo-interface-0.1/1`;
- artefacto: `tondo-artifact-0.1/1`;
- unidad privilegiada: `tondo-privileged-unit-0.1/1`.

Un lector rechaza campos desconocidos. El manifiesto y el lockfile no necesitan
una representación JSON canónica, pero sus bytes exactos participan en los
hashes del build. Las interfaces, artefactos y unidades privilegiadas sí exigen
la codificación canónica compacta producida por el toolchain: sin whitespace
adicional, con los campos en el orden definido por su formato y con todas las
listas identificadoras ordenadas y sin duplicados.

Todos los hashes de este documento se escriben:

~~~text
sha256:<64 dígitos hexadecimales ASCII en minúscula>
~~~

No se aceptan prefijos, algoritmos, mayúsculas ni longitudes alternativas.

### 2.2 Paths

Todo path declarado dentro del proyecto es lógico y relativo:

- utiliza `/`;
- no comienza por `/`;
- no contiene `\`, componentes vacíos, `.` ni `..`;
- no contiene saltos de línea;
- normaliza cada componente a Unicode NFC.

La CLI resuelve el path físico uniendo este path lógico al directorio del
manifiesto. El valor físico resultante no participa en identidad, diagnósticos
ni hashes salvo por los bytes leídos.

Un module path es una secuencia no vacía de identificadores Tondo NFC separada
por `.`. No admite keywords, `_`, `/`, `\` ni componentes vacíos.

Los paths físico-lógico y lógico de toda fuente, incluida una salida generada,
terminan exactamente en `.to`. Interfaces, descriptores y generator inputs no
usan esa restricción.

### 2.3 PackageId y aliases

`PackageId` es una cadena opaca no vacía y sin saltos de línea. El resolvedor de
paquetes que construya un lockfile debe codificar en ella origen, nombre,
versión exacta e integridad cuando correspondan. El compilador compara la cadena
completa byte a byte y nunca interpreta una versión “compatible”.

Un alias y el nombre local de un paquete son identificadores Tondo válidos.
`std` está reservado. Los aliases de un paquete son únicos, no pueden coincidir
con su nombre local y resuelven a un `PackageId` exacto.

## 3. Manifiesto

### 3.1 Forma completa

~~~json
{
  "format": "tondo-manifest-0.1/1",
  "target": {
    "name": "tondo-vm-hosted",
    "profile": "hosted",
    "capability_registry": "tondo-capabilities/1",
    "capabilities": ["console", "process"],
    "features": ["fast"]
  },
  "root": {
    "package": "workspace:app@1",
    "source": "app/src/main.to",
    "form": "module"
  },
  "standard": "toolchain:std:0.1-bootstrap",
  "packages": [
    {
      "id": "workspace:app@1",
      "local_name": "app",
      "edition": "0.1",
      "dependencies": [
        {
          "alias": "util",
          "package": "registry:util@2#sha256-content"
        }
      ],
      "source_sets": [
        {
          "id": "common",
          "when": {},
          "sources": [
            {
              "physical_path": "app/src/main.to",
              "logical_path": "src/main.to",
              "module": "main"
            }
          ]
        }
      ]
    }
  ],
  "generator_inputs": [
    {
      "name": "schema",
      "path": "inputs/schema.json"
    }
  ],
  "privileged_units": [
    {
      "name": "vendor.native",
      "path": "units/vendor-native.tpu"
    }
  ]
}
~~~

`generator_inputs` y `privileged_units` pueden omitirse y equivalen a listas
vacías. `dependencies`, `capabilities`, `features` y `when` también admiten su
forma vacía según el schema. Los demás campos son obligatorios.

El nombre de un generator input es no vacío, no contiene saltos de línea y no
puede comenzar por `privileged:`. Ese prefijo está reservado para registrar
unidades privilegiadas sin colisiones en la identidad del artefacto.

### 3.2 Target

El registro inicial es `tondo-capabilities/1` y contiene exactamente:

~~~text
clock
console
dynamic-linking
entropy
environment
filesystem
network
process
threads
~~~

Un nombre fuera de este registro es error, incluso si no se utiliza. Un target
solo puede seleccionar capacidades que implemente realmente.

El bootstrap implementa una única combinación:

~~~text
target       = tondo-vm-hosted
profile      = hosted
capabilities = console, process
~~~

Puede compilarse con cualquier subconjunto de esas dos capacidades. Solicitar
otra capacidad registrada, como `network`, es error de configuración del
target. La ausencia de `console` o `process` elimina el correspondiente módulo
de `std`; importarlo produce `E1008`.

Los nombres de feature son identificadores kebab-case ASCII: comienzan por una
letra minúscula y continúan con minúsculas, dígitos o `-`. El lenguaje no les
atribuye semántica implícita.

### 3.3 Raíz

`root.package` debe nombrar exactamente un elemento de `packages`.
`root.source` debe ser uno de sus `physical_path`. `root.form` es exactamente
uno de:

| Valor | Interpretación |
|---|---|
| `module` | módulo ejecutable ordinario |
| `script` | raíz con sentencias top-level y `main` implícito |
| `fragment` | fragmento de tooling |

La fuente raíz debe pertenecer a un source set activo. Una raíz declarada pero
inactiva es error del proyecto.

### 3.4 Paquetes

Cada paquete declara:

- un `PackageId` único;
- un nombre local;
- exactamente la edición `0.1` en este bootstrap;
- aliases de dependencias directas;
- uno o más source sets.

El paquete estándar es propiedad del toolchain y no aparece en `packages`. El
bootstrap exige:

~~~text
PackageId = toolchain:std:0.1-bootstrap
~~~

El grafo resultante debe ser cerrado y acíclico. Toda dependencia debe existir
en `packages`; no hay búsqueda por nombre, directorio ni registry durante la
compilación.

## 4. Source sets

### 4.1 Condición

El objeto opcional `when` admite:

~~~json
{
  "targets": ["tondo-vm-hosted"],
  "profiles": ["hosted"],
  "requires_capabilities": ["process"],
  "excludes_capabilities": ["network"],
  "requires_features": ["fast"],
  "excludes_features": ["debug-api"]
}
~~~

Una lista vacía de targets o perfiles no restringe. Un source set queda activo
si:

- target y perfil aparecen en sus listas cuando estas no están vacías;
- todas las capacidades y features requeridas están seleccionadas;
- ninguna capacidad o feature excluida está seleccionada.

Una misma capacidad o feature no puede aparecer a la vez en `requires_*` y
`excludes_*`. Un target, perfil o capacidad desconocido se rechaza aunque la
condición no fuera a activar el set.

### 4.2 Selección preléxica

El plan evalúa todas las condiciones antes de solicitar bytes de fuente. Solo
las fuentes de sets activos aparecen en `required_inputs`. Por tanto:

- un archivo inactivo no se abre;
- UTF-8 inválido o sintaxis rota en un archivo inactivo no llega al lexer;
- no existe `#if` ni resolución condicional dentro de `.to`.

El ID global de un source set utiliza longitud UTF-8 para que un `PackageId`
opaco que contenga `#` no pueda colisionar con otro paquete:

~~~text
@<bytes-de-PackageId>:<PackageId>#<source-set-id>
~~~

Por ejemplo, `workspace:app@1` y el set `common` producen
`@15:workspace:app@1#common`. La longitud usa bytes UTF-8 y su decimal canónico,
sin ceros iniciales.

El `source-set-id` local es kebab-case ASCII. Dos fuentes activas del mismo
paquete no pueden compartir `logical_path`. Cada `physical_path` de fuente
declarado es único en todo el manifiesto, activo o no.

Varios archivos activos pueden contribuir al mismo módulo si conservan paths
lógicos distintos y sus declaraciones no colisionan.

## 5. Lockfile

### 5.1 Forma completa

~~~json
{
  "format": "tondo-lock-0.1/1",
  "manifest_hash": "sha256:...",
  "standard": {
    "package_id": "toolchain:std:0.1-bootstrap",
    "content_hash": "sha256:..."
  },
  "packages": [
    {
      "id": "workspace:app@1",
      "content_hash": "sha256:...",
      "dependencies": [
        {
          "alias": "util",
          "package": "registry:util@2#sha256-content"
        }
      ],
      "sources": [
        {
          "source_set": "common",
          "physical_path": "app/src/main.to",
          "logical_path": "src/main.to",
          "module": "main",
          "sha256": "sha256:..."
        }
      ],
      "interface": null
    },
    {
      "id": "registry:util@2#sha256-content",
      "content_hash": "sha256:...",
      "dependencies": [],
      "sources": [],
      "interface": {
        "path": "interfaces/util.ti",
        "sha256": "sha256:..."
      }
    }
  ],
  "generator_inputs": [
    {
      "name": "schema",
      "sha256": "sha256:..."
    }
  ],
  "privileged_units": [
    {
      "name": "vendor.native",
      "sha256": "sha256:..."
    }
  ]
}
~~~

### 5.2 Correspondencia exacta

`manifest_hash` es SHA-256 de los bytes exactos del manifiesto. Los conjuntos de
paquetes, fuentes, dependencies, generator inputs y unidades privilegiadas del
lockfile deben coincidir exactamente con el manifiesto:

- mismo `PackageId`;
- mismo alias y destino exacto;
- mismo source set, path físico, path lógico y módulo;
- mismo nombre de entrada;
- ningún elemento adicional ni ausente.

La raíz no consume su propia interfaz. Todo paquete no raíz y no estándar debe
tener una interfaz fijada. El hash de la interfaz participa además en el hash de
contenido de ese paquete.

### 5.3 Hash de paquete

Para calcular `content_hash`, el toolchain:

1. ordena dependencies por `(alias, PackageId)`;
2. ordena sources por `physical_path`;
3. codifica de forma JSON compacta este record, en este orden de campos:

~~~text
{
  package_id,
  dependencies,
  sources,
  interface_hash
}
~~~

4. aplica SHA-256 a esos bytes.

`dependencies` conserva los campos `alias`, `package`; cada source conserva,
en orden, `source_set`, `physical_path`, `logical_path`, `module`, `sha256`.
`interface_hash` es el hash de bytes de la interfaz o `null` para la raíz.

El paquete estándar bootstrap utiliza el fingerprint fijo publicado por esa
versión del compilador. Un hash estándar distinto se rechaza.

## 6. Resolución cerrada

La API pura sigue dos pasos:

1. `ProjectPlan::parse(manifest, lockfile)` valida identidades, condiciones y
   grafo, y devuelve la lista exacta de entradas requeridas.
2. `ProjectPlan::resolve(supplied)` acepta un mapa path → bytes.

Cada entrada requerida tiene kind y SHA-256:

| Kind | Contenido |
|---|---|
| `source` | fuente de un source set activo |
| `dependency-interface` | interfaz compilada fijada |
| `generator-input` | dato declarado para generación previa |
| `privileged-unit` | descriptor privilegiado canónico |

Falta, sobra o cambia un byte de una entrada y la resolución falla. Ninguna
entrada inactiva se solicita. Tras validar todos los hashes, el plan construye
determinísticamente `SourceDatabase`, `PackageGraph` y `CompilationRequest`.

Los generator inputs ya deben haber producido, fuera del compilador, cualquier
fuente declarada por el manifiesto. El compilador registra sus hashes, pero no
ejecuta generadores.

## 7. Interfaces compiladas

### 7.1 Propósito

Una interfaz identifica la superficie Tondo pública de un paquete y las
condiciones exactas bajo las que se comprobó. No es bytecode, no contiene
layout, no fija name mangling y no promete una ABI.

### 7.2 Schema canónico

~~~json
{
  "format": "tondo-interface-0.1/1",
  "compiler": "tondo-bootstrap/0.0.0",
  "edition": "0.1",
  "package_id": "registry:util@2#sha256-content",
  "target": "tondo-vm-hosted",
  "profile": "hosted",
  "capability_registry": "tondo-capabilities/1",
  "capabilities": ["console", "process"],
  "features": ["fast"],
  "source_sets": ["@30:registry:util@2#sha256-content#common"],
  "modules": ["util"],
  "api_hash": "sha256:...",
  "dependencies": [
    {
      "alias": "core",
      "package_id": "registry:core@1#sha256-content",
      "api_hash": "sha256:..."
    }
  ]
}
~~~

Capacidades, features, source sets, módulos y aliases de dependencias están
ordenados y no se repiten. `source_sets` contiene solo los sets activos del
paquete descrito, no los de su consumidor. Cada dependencia directa conserva
alias, `PackageId` completo y hash de API; la cadena de interfaces fija así el
grafo transitivo.

`api_hash` se deriva de la superficie pública canónica: nombres nominales,
signatures, genéricos y bounds, constantes públicas, formas nominales,
capacidades derivadas observables, traits, métodos e implementaciones
publicables. Los detalles privados no se serializan; un detalle privado que
cambie una propiedad pública sí cambia el hash.

### 7.3 Admisión

Antes de lexear cualquier fuente, el consumidor comprueba:

- versión de formato y registro de capacidades;
- identidad exacta de compilador;
- edición del paquete;
- `PackageId`;
- target y perfil;
- conjunto de capacidades y features;
- source sets activos de ese paquete;
- conjunto de módulos;
- aliases, `PackageId` y hashes de API de dependencias transitivas.

Tras comprobar la fuente seleccionada del paquete, el build vuelve a derivar su
API pública y exige que coincida con `api_hash`. Esta verificación adicional del
bootstrap evita que una interfaz fijada describa fuentes distintas.

Una discrepancia nunca intenta enlazar “por parecido” ni cae a búsqueda nominal.

## 8. Artefacto de build

### 8.1 Schema canónico

~~~json
{
  "format": "tondo-artifact-0.1/1",
  "compiler": "tondo-bootstrap/0.0.0",
  "edition": "0.1",
  "source_form": "module",
  "package_id": "workspace:app@1",
  "target": "tondo-vm-hosted",
  "profile": "hosted",
  "capability_registry": "tondo-capabilities/1",
  "capabilities": ["console", "process"],
  "features": ["fast"],
  "source_sets": [
    "@15:workspace:app@1#common",
    "@30:registry:util@2#sha256-content#common"
  ],
  "manifest_hash": "sha256:...",
  "lockfile_hash": "sha256:...",
  "generator_inputs": {
    "privileged:vendor.native": "sha256:...",
    "schema": "sha256:..."
  },
  "source_hashes": [
    {
      "source_id": "pkg:...",
      "module": "main",
      "path": "src/main.to",
      "sha256": "sha256:..."
    }
  ],
  "interface_hash": "sha256:...",
  "build_hash": "sha256:...",
  "reproducible": true
}
~~~

El artefacto registra todos los source sets del grafo seleccionado, no solo los
de la raíz. `source_hashes` se ordena por `(source_id, module, path)`.
`generator_inputs` incluye las entradas ordinarias por nombre y cada unidad
privilegiada como `privileged:<id>`.

`build_hash` es SHA-256 de una codificación canónica que contiene, en orden:
compilador, edición, source form, PackageId raíz, target, perfil, capacidades,
features, source sets, hashes de manifiesto y lockfile, generator inputs, source
hashes e interface hash.

`reproducible: true` solo es válido porque todas esas entradas son explícitas y
el compilador puro no accede a estado ambiental.

## 9. Unidades privilegiadas

### 9.1 Alcance

Una unidad privilegiada fija la identidad de un adaptador nativo o intrinsic y
los contratos que expone. No añade `extern`, atributos arbitrarios ni pragmas a
`.to`. Tampoco declara una ABI nativa estable: una ABI general requiere su
propia especificación de layout, calling convention, ownership, callbacks,
roots, threads y unwind.

### 9.2 Schema canónico

~~~json
{
  "format": "tondo-privileged-unit-0.1/1",
  "id": "vendor.native",
  "provider": "registry:vendor-native@1#sha256-content",
  "compiler": "tondo-bootstrap/0.0.0",
  "target": "tondo-vm-hosted",
  "profile": "hosted",
  "capability_registry": "tondo-capabilities/1",
  "required_capabilities": ["process"],
  "bindings": [
    {
      "canonical_name": "vendor.native.checkedHandle",
      "exposure": "safe-wrapper",
      "signature_hash": "sha256:...",
      "safety_contract_hash": "sha256:...",
      "implementation_hash": "sha256:..."
    },
    {
      "canonical_name": "vendor.native.rawHandle",
      "exposure": "unsafe-function",
      "signature_hash": "sha256:...",
      "safety_contract_hash": "sha256:...",
      "implementation_hash": "sha256:..."
    }
  ]
}
~~~

`id` es una secuencia de identificadores kebab-case separados por `.`.
`provider` es un `PackageId`. Capacidades y bindings están ordenados y no se
repiten.

Cada binding fija tres dimensiones distintas:

- `signature_hash`: contrato estático Tondo;
- `safety_contract_hash`: precondiciones del caller o invariantes demostradas
  por el wrapper;
- `implementation_hash`: implementación exacta del adaptador.

`unsafe-function` deja obligaciones explícitas al llamador. `safe-wrapper`
afirma que el adaptador valida o encapsula todas las precondiciones y no deja
escapar un estado inválido a Tondo seguro.

El plan rechaza una unidad con compiler, target, perfil, registro o capacidad
incompatible antes de compilar fuente. Su hash exacto forma parte del artefacto.

## 10. CLI bootstrap

Las formas de proyecto son:

~~~text
tondo check --manifest <tondo.json>
tondo run --manifest <tondo.json> -- [argument ...]
~~~

Opciones:

~~~text
--lockfile <path>        lockfile explícito
--emit-interface <path>  interfaz canónica de la raíz
--emit-artifact <path>   metadatos canónicos del build
~~~

Sin `--lockfile`, se utiliza `tondo.lock.json` junto al manifiesto. `fmt` sigue
operando sobre un único `.to` y no acepta manifiestos ni productos.

La CLI:

1. lee manifiesto y lockfile;
2. crea el plan puro;
3. lee exactamente `required_inputs`, relativos al manifiesto;
4. entrega los bytes al plan;
5. compila;
6. escribe productos solo si la compilación tiene éxito.

Un producto no puede declarar el mismo path que una fuente suelta, manifiesto,
lockfile, source activo, interfaz de dependencia, generator input, unidad
privilegiada u otro producto de esa invocación.

## 11. Determinismo y frontera ambiental

Dos ejecuciones con bytes idénticos de:

- manifiesto y lockfile;
- fuentes activas;
- interfaces;
- generator inputs;
- unidades privilegiadas;
- misma versión de compilador;

producen bytes idénticos de interfaz y artefacto.

El orden físico de lectura no modifica source IDs, módulos, API hash, orden de
diagnósticos ni build hash. Todo set o mapa que cruza una frontera serializada
se ordena por su identidad canónica.

El módulo de planificación del compilador no importa ni invoca APIs de
filesystem, entorno, red, proceso o reloj. Un frontend diferente puede obtener
los bytes desde memoria, un sandbox o un content-addressed store sin cambiar la
compilación.

## 12. Errores de frontera

Los errores de manifiesto, lockfile, hashes, interfaces y unidades privilegiadas
son errores del toolchain, no diagnósticos de fuente Tondo. Ocurren antes de
lexear cuando la incompatibilidad ya es visible.

Los errores de fuente conservan sus códigos normativos. En particular:

- `E1008`: módulo o API ausente por target/capacidad;
- `E1701`: llamada u operación raw fuera de una región `unsafe`;
- `E1702`: captura de `Pointer` por un cierre seguro.

No se fabrica un span de fuente para un error del grafo cerrado.

## 13. Límites deliberados de 0.1

Este contrato no define:

- resolución semver o acceso a registries;
- comandos para actualizar el lockfile;
- ejecución de generadores;
- cache remota o incremental;
- firma criptográfica de paquetes;
- formato de bytecode persistente;
- linker nativo;
- ABI FFI general;
- compatibilidad de interfaces entre compiladores distintos.

Añadir cualquiera de esas piezas no puede relajar la entrada cerrada del
compilador ni convertir estado ambiental no declarado en semántica fuente.
