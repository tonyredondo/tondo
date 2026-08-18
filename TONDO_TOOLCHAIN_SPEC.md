# Tondo Toolchain 0.1

**Estado:** borrador normativo en desarrollo; Tondo todavía no se ha publicado

**Versión:** `tondo-toolchain-draft`

**Especificación de lenguaje:** [Tondo 0.1](./TONDO_LANGUAGE_SPEC.md)

**Especificación de testing:** [Testing Tondo 0.1](./TONDO_TESTING_SPEC.md)

**Formato para LLMs:** [Tondo LLM Form](./TONDO_LLM_FORM_SPEC.md)

Esta especificación define la frontera de proyecto del toolchain Tondo 0.1:
layout convencional, configuración TOML opcional, lockfile, selección de
fuentes, grafo de paquetes, interfaces compiladas, metadatos de artefacto y
unidades privilegiadas. No modifica la sintaxis ni la semántica de un archivo
`.to`.

Las palabras **debe**, **no debe**, **puede** y **error** son normativas para el
toolchain del draft. La resolución de versiones, descarga de
paquetes y una ABI nativa general permanecen fuera de este contrato. La
generación hermética en compile time sí forma parte de él; su implementación se
rastrea dentro de la misma línea de desarrollo; no existe todavía una promesa
de compatibilidad entre releases.

## 1. Objetivos

La frontera de proyecto tiene cinco propiedades:

1. El compilador recibe un grafo cerrado; nunca descubre paquetes o archivos.
2. Target, perfil, capacidades, features y source sets son entradas explícitas.
3. Toda entrada física se declara y queda fijada por SHA-256 antes de analizar
   fuente.
4. Una interfaz incompatible se rechaza antes de lexear o typecheckear el
   consumidor.
5. Entradas idénticas producen bytes idénticos de fuente generada, interfaz y
   artefacto.

El compilador no consulta red, reloj, variables de entorno, directorio actual,
aleatoriedad ni procesos. La CLI puede descubrir un proyecto por sus
convenciones y materializar un grafo interno; después solo entrega al
compilador los `required_inputs` cerrados. El JSON del compilador sigue siendo
un formato interno, no una configuración que el usuario deba mantener.

`tondo test` añade una fase de orquestación definida por la especificación de
testing: puede enumerar únicamente las convenciones dentro de roots declarados,
normaliza y hashea el resultado y construye después un plan cerrado con clases
`production`, `unit-test` e `integration-test`. El frontend nunca descubre
archivos, y `check`/`run` no adquieren esta excepción acotada.

## 2. Identidades y codificación

### 2.1 Frontera JSON interna

Los records internos del compilador se serializan con UTF-8 JSON únicamente
dentro de la frontera pura; no son archivos de proyecto ni formatos de entrada
de la CLI. Los únicos archivos persistentes de configuración son `tondo.toml`,
`tondo.lock.toml` y, opcionalmente, `tondo.test.toml`:

- manifiesto: `tondo-manifest-draft`;
- lockfile: `tondo-lock-draft`;
- interfaz: `tondo-interface-draft`;
- artefacto: `tondo-artifact-draft`;
- descriptor estándar: `tondo-standard-descriptor-draft`;
- unidad privilegiada: `tondo-privileged-unit-draft`.

Un lector interno rechaza campos desconocidos. Los records de proyecto no necesitan
una representación JSON canónica, pero sus bytes exactos participan en los
hashes del build. Las interfaces, artefactos y unidades privilegiadas sí exigen
la codificación canónica compacta producida por el toolchain: sin whitespace
adicional, con los campos en el orden definido por su formato y con todas las
listas identificadoras ordenadas y sin duplicados. El descriptor estándar usa
también esa codificación canónica.

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

El prefijo lógico `@generated/` está reservado al toolchain. No puede aparecer
en `physical_path`, fuentes escritas ni outputs de un generador general. Cada
derive recibe:

~~~text
@generated/derive/<request-hash-sin-prefijo>.to
~~~

Su módulo es exactamente el módulo propietario del target. El hash hexadecimal
minúsculo evita colisiones sin introducir nombres elegidos por el provider.

El source ID de una expansión es `derive:<64hex>` y el de un output general es
`gen:<64hex>:<index>`, donde `<64hex>` es el request hash sin `sha256:` e
`<index>` es su posición decimal sin ceros iniciales en la lista canónica de
outputs. Estos namespaces no colisionan con IDs de paquetes o targets.

### 2.3 PackageId y aliases

`PackageId` es una cadena opaca no vacía y sin saltos de línea. El resolvedor de
paquetes que construya un lockfile debe codificar en ella origen, nombre,
versión exacta e integridad cuando correspondan. El compilador compara la cadena
completa byte a byte y nunca interpreta una versión “compatible”.

Un alias y el nombre local de un paquete son identificadores Tondo válidos.
`std` está reservado. Los aliases de un paquete son únicos, no pueden coincidir
con su nombre local y resuelven a un `PackageId` exacto.

## 3. Proyectos convencionales y configuración humana

La CLI normal es *convention-first*: una aplicación no necesita ningún
manifiesto JSON para compilarse. El directorio del proyecto es el directorio actual o el
argumento `--project <dir>`. La forma recomendada es:

~~~text
app/
  src/
    main.to
    models/user.to
  tests/
    user_test.to
  tondo.toml                 # opcional
  tondo.lock.toml            # solo si hay dependencias externas
~~~

Las reglas de descubrimiento de producción son deterministas:

1. Si existe `src/`, se recorren sus archivos `.to` excepto `*_test.to`.
   `src/main.to` es la raíz preferida; si no existe, la raíz es el primer path
   físico de producción ordenado por bytes UTF-8. El árbol `tests/` no forma
   parte de este source set.
2. Sin `src/`, un proyecto reconocido debe tener `tondo.toml` o `main.to`.
   En ese caso se incluyen los `.to` del nivel raíz excepto `*_test.to`. Un
   directorio sin ninguno de esos marcadores no se trata como proyecto aunque
   contenga archivos `.to` auxiliares.
3. Se ignoran symlinks, directorios ocultos, `target/` y `vendor/`. Los paths
   se normalizan a `/`, son relativos a la raíz y forman el `physical_path`,
   `logical_path` y módulo del manifiesto interno.
4. El nombre del paquete se toma de `[package].name`; si falta, se usa el
   nombre del directorio y debe ser un alias Tondo válido. Un directorio con
   guiones u otro spelling no válido debe declarar el nombre explícitamente.

`check`, `build` y `run` consumen únicamente este conjunto de producción.
`tondo test` parte del mismo conjunto y añade después, por su frontera exclusiva
de discovery, los `*_test.to` bajo `src/` o el nivel raíz y los `.to` bajo
`tests/`; esa ampliación se clasifica antes de construir el plan cerrado y nunca
contamina un artefacto de producción. `tondo.lock.toml` fija únicamente el
manifest y las fuentes de producción. El comando de test valida primero ese
lock sin modificarlo y deriva en memoria un lock cerrado del overlay, con hash
del manifest, hashes de fuentes y content hash del paquete recalculados; no
existe un segundo lockfile mantenido por el usuario.

`tondo.toml` es la única configuración humana del proyecto. No repite fuentes
ni módulos. Sus tablas admitidas son:

~~~toml
[package]
name = "app"
edition = "0.1"

[target]
name = "tondo-vm-hosted"
profile = "hosted"
capability_registry = "tondo-capabilities-draft"
capabilities = ["console", "process"]
features = []

[dependencies]
http = "registry:http@1"
serde = { package = "registry:serde@1", interface = "interfaces/serde.ti" }
~~~

Todas las tablas y claves desconocidas son error. Si no existe `[target]`, se
usan `tondo-vm-hosted`, perfil `hosted`, el registro vigente, las capacidades
hosted soportadas y features vacías. Si no existe `[package]`, la edición es
`0.1` y el nombre se deriva según las reglas anteriores.

Las dependencias externas requieren `tondo.lock.toml`, que es un artefacto
generado por el resolvedor y debe fijar el mismo grafo, hashes de fuente,
interfaces y standard package que el manifiesto interno. El compilador no
resuelve versiones ni red. Un proyecto sin dependencias puede omitir el
lockfile: la CLI materializa un lockfile equivalente en memoria. El formato
TOML del lockfile conserva estructuralmente las tablas `standard`, `packages`,
`sources`, `generator_inputs` y `privileged_units`. La CLI no lee ni escribe
manifiestos JSON de proyecto: genera los registros internos necesarios en
memoria y los entrega a la frontera pura del compilador. Esa representación
interna no es un archivo de configuración ni una API de usuario.

### 3.1 Manifiesto interno: forma completa

La forma siguiente es una representación privada del compilador para hashes,
interfaces y herramientas. No corresponde a un archivo que un proyecto pueda
crear o pasar a `tondo`; la configuración humana es exclusivamente TOML.

~~~json
{
  "format": "tondo-manifest-draft",
  "target": {
    "name": "tondo-vm-hosted",
    "profile": "hosted",
    "capability_registry": "tondo-capabilities-draft",
    "capabilities": ["console", "process"],
    "features": ["fast"]
  },
  "root": {
    "package": "workspace:app@1",
    "source": "app/src/main.to",
    "form": "module"
  },
  "standard": "toolchain:std:draft",
  "meta_packages": [],
  "packages": [
    {
      "id": "workspace:app@1",
      "local_name": "app",
      "edition": "0.1",
      "dependencies": [],
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
  "generators": [
    {
      "id": "models",
      "owner_package": "workspace:app@1",
      "provider": {
        "package": "toolchain:std-meta:draft",
        "entry": "schema.generateModels"
      },
      "meta_model": "tondo-meta-model-0.1/1",
      "inputs": ["schema"],
      "model_roots": [],
      "outputs": [
        {
          "logical_path": "generated/models.to",
          "module": "models"
        }
      ],
      "limits": {
        "steps": 10000000,
        "memory_bytes": 67108864,
        "output_bytes": 8388608
      }
    }
  ],
  "derive_providers": [],
  "privileged_units": [
    {
      "name": "vendor.native",
      "path": "units/vendor-native.tpu"
    }
  ]
}
~~~

`meta_packages`, `generator_inputs`, `generators`, `derive_providers` y
`privileged_units` pueden omitirse y equivalen a listas vacías. `dependencies`,
`capabilities`, `features`, `model_roots` y `when` también admiten su forma
vacía según el schema. Los demás campos son obligatorios.

El nombre de un generator input es no vacío, no contiene saltos de línea y no
puede comenzar por `privileged:`. Ese prefijo está reservado para registrar
unidades privilegiadas sin colisiones en la identidad del artefacto.

Un `generator.id` es kebab-case ASCII y único. `owner_package` debe existir en
`packages`; todas sus salidas pertenecen a ese paquete. `provider.package` debe
existir en el grafo meta cerrado de 3.5 o ser su paquete estándar exacto.
`entry` es un value path Tondo. `inputs` no repite nombres y solo contiene
elementos de `generator_inputs`.

`model_roots` es una lista ordenada y sin duplicados de objetos
`{"package": PackageId, "module": module_path}`. Cada elemento debe existir en
el grafo runtime y selecciona la clausura pública que recibirá el generator. La
lista vacía entrega un modelo sin declaraciones y es la forma normal de
generación schema-first. Un root que dependa transitivamente de fuente generada
en esa misma ronda produce `E2109`.

Los outputs se ordenan por `(logical_path, module)`, no se repiten y no
colisionan con fuentes activas ni con outputs de otro productor. Los tres límites
son enteros decimales positivos. Sus unidades son pasos deterministas del VM
meta, bytes máximos de memoria viva y bytes de la codificación canónica completa
de la respuesta antes de formatear, incluidos fuente, diagnostics y source maps.

Una entrada de `derive_providers` identifica un provider **adicional** del
proyecto mediante la tupla exacta `(PackageId, module path, declaration name)`.
Su forma es:

~~~json
{
  "trait": {
    "package": "workspace:domain@1",
    "module": "model",
    "name": "Validate"
  },
  "provider": {
    "package": "workspace:domain-meta@1",
    "entry": "validation.expand"
  },
  "meta_model": "tondo-meta-model-0.1/1",
  "limits": {
    "steps": 1000000,
    "memory_bytes": 16777216,
    "output_bytes": 1048576
  }
}
~~~

La distribución estándar seleccionada aporta sus propios mappings —por ejemplo
`Serialize`— desde un descriptor fijado; el proyecto no los repite. La unión de
ambos registros no puede contener dos entradas para la misma identidad de trait.
El provider, modelo y límites siguen las mismas reglas que un generador. Un
provider solo se ejecuta cuando la fuente contiene una solicitud `derive` para
esa identidad. Si el trait es genérico, el mapping pertenece a su declaración y
el request conserva por separado los argumentos concretos escritos.

### 3.2 Target

El registro inicial es `tondo-capabilities-draft` y contiene exactamente:

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

El paquete estándar es propiedad del toolchain y no aparece en `packages`. La
distribución actual del draft utiliza:

~~~text
PackageId = toolchain:std:draft
~~~

El corpus bootstrap de regresión utiliza `toolchain:std:0.1-bootstrap`. Es una
entrada de prueba heredada y no una segunda edición del toolchain; el lector
actual nunca acepta un formato alternativo por compatibilidad implícita.

El grafo resultante debe ser cerrado y acíclico. Toda dependencia debe existir
en `packages`; no hay búsqueda por nombre, directorio ni registry durante la
compilación.

### 3.5 Grafo meta

`meta_packages` forma un segundo grafo cerrado, separado de `packages`. Cada
elemento declara:

- `id`, `local_name` y edición `0.1`;
- dependencies que solo pueden apuntar a otros `meta_packages`; y
- una lista no vacía `sources` con `physical_path`, `logical_path` y `module`.

Los PackageIds de ambos grafos son disjuntos. Los paths físicos son únicos en
todo el manifiesto y las fuentes meta siguen las mismas reglas lógicas de 2.2,
pero no usan source sets ni condiciones: siempre se compilan para
`target = tondo-meta`, `profile = meta` y cero capabilities. Un paquete meta
inaccesible desde algún `provider.package` se rechaza como input sobrante.

La distribución runtime seleccionada por `standard` declara en su descriptor un
único paquete estándar meta compatible. Para
`toolchain:std:draft` es:

~~~text
PackageId = toolchain:std-meta:draft
~~~

El descriptor `tondo-standard-descriptor-draft` forma parte de los bytes
cubiertos por el `content_hash` runtime. Contiene exactamente el PackageId y
content hash del companion meta y el registro ordenado de providers estándar;
cada entrada conserva identidad de trait, PackageId/entry del provider, versión
de modelo, `provider_hash` y límites. No contiene defaults ambientales ni
aliases de proyecto.

El proyecto no repite esa asociación en el manifiesto. El lockfile materializa
ambas identidades y hashes para que la selección siga siendo explícita y
auditable. Dentro del grafo meta, `std` resuelve a ese paquete; nunca al
`standard` runtime.

La codificación concreta del descriptor es compacta y con campos en este orden:

~~~json
{
  "format": "tondo-standard-descriptor-draft",
  "runtime": {
    "package_id": "toolchain:std:draft",
    "content_hash": "sha256:..."
  },
  "meta": {
    "package_id": "toolchain:std-meta:draft",
    "content_hash": "sha256:..."
  },
  "derive_providers": []
}
~~~

Cada elemento de `derive_providers` usa la misma forma expandida del lockfile y
debe llevar `origin: "standard"`. El lector actual expone este contrato como
`StandardDescriptor`; `ProjectPlanDraft::parse` exige que `standard`, `meta_standard`
y todos los providers estándar del lockfile coincidan con él antes de enumerar
fuentes.

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

### 4.3 Plan cerrado de testing

Una invocación `tondo test` puede consumir un record independiente
`tondo-test-plan-draft`, pero no obliga al usuario a mantenerlo. Sin sidecar,
el toolchain materializa en memoria este mismo shape con defaults opinionados a
partir del `ProjectPlan` ya cerrado. `--test-plan <path>` selecciona un record
explícito; si no se proporciona, un `tondo.test.toml` adyacente se usa cuando
existe. No existe una variante JSON de este sidecar y la CLI no acepta una. No cambia el
manifiesto de producción ni crea un segundo parser. Todo
record suministrado se valida contra el `manifest_hash` y `lockfile_hash`
exactos de un `ProjectPlan` ya cerrado, y todos sus campos son inputs del plan
de test. El ejemplo siguiente muestra la forma interna; el único sidecar que
puede escribir el usuario es su equivalente TOML.

~~~json
{
  "format": "tondo-test-plan-draft",
  "project": {
    "manifest_hash": "sha256:...",
    "lockfile_hash": "sha256:..."
  },
  "repository_root": "",
  "roots": [
    {"class":"production", "physical_path":"app/src", "logical_path":"src"},
    {"class":"unit-test", "physical_path":"app/src", "logical_path":"src"},
    {"class":"integration-test", "physical_path":"tests", "logical_path":"tests"}
  ],
  "sources": [
    {
      "class":"production", "package":"workspace:app@1",
      "physical_path":"app/src/main.to", "logical_path":"src/main.to",
      "module":"main", "input":"source:production:app/src/main.to"
    }
  ],
  "dev_dependencies": [],
  "codeowners": {"mode":"auto"},
  "selector": {"kind":"none"},
  "shard": null,
  "order": {"kind":"canonical"},
  "policy": {"jobs":1, "allow_empty":false, "fail_fast":false,
              "retry":0, "repeat":1},
  "reporters": ["human", "json"],
  "artifact_store": {"path":"target/test-artifacts",
                      "content_addressed":true, "max_bytes":1048576},
  "snapshot_stores": [],
  "target": {"name":"tondo-vm-hosted", "profile":"hosted",
              "capability_registry":"tondo-capabilities-draft",
              "capabilities":["console", "process"], "features":[]},
  "time_catalog": {"package":"std", "module":"time", "api":"monotonic-v1"},
  "limits": {"timeout_ms":1000, "setup_timeout_ms":1000,
             "teardown_timeout_ms":1000, "output_bytes":65536,
             "artifact_bytes":1048576, "snapshot_bytes":1048576,
             "memory_bytes":67108864, "instructions":1000000,
             "virtual_timers":1024}
}
~~~

Sin record suministrado, el plan en memoria usa `codeowners: auto`, selector
vacío, orden canónico, `jobs: 1`, `retry: 0`, `repeat: 1`,
`target/test-artifacts` content-addressed y ningún snapshot store. Sus defaults
de límites son 30 s para cada timeout, 1 MiB de output, 16 MiB para artifacts y
snapshots, 64 MiB de memoria, 10.000.000 de instrucciones y 1.024 timers
virtuales. Las fuentes activas del `ProjectPlan` se registran como
`production`; sus roots se derivan únicamente de sus paths canónicos.

`class` es exactamente `production`, `unit-test` o `integration-test`. Cada
fuente pertenece a una única clase, tiene un nombre de input único y debe estar
cubierta por una raíz explícita de la misma clase; nunca se deduce una raíz por
common-prefix. Las fuentes `production` deben coincidir byte a byte en path
físico con los source sets activos del `ProjectPlan`. Una fuente de integración
puede usar un `PackageId` sintético; las demás deben pertenecer al grafo cerrado.
Los paths del record son lógicos, relativos, slash-separated y no pueden
escapar de la raíz del repositorio. El valor canónico de `repository_root` es
la cadena vacía, que representa la raíz; un source root también puede usar la
cadena vacía para representar todo el repositorio. `.` se acepta solo como
entrada y se normaliza a `""`.

Las dependencias de desarrollo contienen `alias`, `PackageId`, path de interfaz
y SHA-256. Son una lista separada de las dependencias de producción y no pueden
aparecer en el artefacto de producción. Su materialización y la clasificación
de inputs públicos o secretos pertenecen a `UTEST-INPUTS-PLAN-001`; el plan
solo fija sus nombres y referencias.

`codeowners.mode` es `auto`, `none` o `path` (este último exige `path`). El
runner no abre el archivo durante esta fase. `selector.kind` es `none`,
`filter`, `glob` o `exact`; solo los tres últimos llevan `value` y son
mutuamente excluyentes. `shard` es `null` o `{index,count}` con
`1 <= index <= count`. `order` es `canonical` o `random`; una seed solo es
válida para `random`, se normaliza a dieciséis dígitos hexadecimales y puede
estar ausente para solicitar entropía explícita durante la materialización.

La política fija `jobs > 0`, `repeat > 0`, `retry >= 0`, `allow_empty` y
`fail_fast`. Reporters son un conjunto no vacío de `human`, `json` y `junit`.
Los argumentos de una invocación pueden sobreescribir selector, shard,
orden/seed, jobs, retry, repeat y la presentación/salida de la campaña sin
modificar este record. Los reporters estructurales, techos de recursos, target,
capabilities, dependencias y formatos de stores no se pueden ampliar mediante
esos overrides.
El artifact store es siempre content-addressed; snapshot stores tienen nombre,
path, flag de actualización y límite independiente. El target, capabilities y
features deben coincidir exactamente con el proyecto y el catálogo temporal
único de esta edición es `std.time@monotonic-v1`. Todos los límites de trabajo,
memoria, output, artifacts, snapshots y timers son enteros positivos.

El lector puro normaliza listas y seeds, rechaza campos desconocidos,
duplicados, hashes inválidos, roots ausentes, deriva de producción,
capabilities/target/time-base incompatibles y presupuestos cero. Su salida
canónica no contiene bytes de fuentes, valores de inputs ni datos de
CODEOWNERS. `TestProjectPlan::parse` implementa exactamente esta frontera;
discovery, inputs, ownership, compilación y workers son tareas posteriores.

### 4.4 Plan de inputs sin materialización

El plan de proyecto puede referenciar inputs mediante `source.input`, pero esos
nombres solo quedan cerrados cuando se valida un record independiente
`tondo-test-input-plan-draft`. La validación recibe el `TestProjectPlan` ya
normalizado y nunca abre un archivo, consulta el host, resuelve un provider ni
materializa un valor. Su forma canónica es:

~~~json
{
  "format": "tondo-test-input-plan-draft",
  "test_plan_sha256": "sha256:...",
  "inputs": [
    {
      "name": "source:production:app/src/main.to",
      "source": "app/src/main.to",
      "profile": "build",
      "visibility": "public",
      "sha256": "sha256:...",
      "provider": null,
      "descriptor": null,
      "version": null,
      "capability": null
    },
    {
      "name": "host:token",
      "source": "environment:TOKEN",
      "profile": "runtime",
      "visibility": "secret",
      "sha256": null,
      "provider": "ci",
      "descriptor": "TOKEN",
      "version": "v1",
      "capability": "environment"
    }
  ],
  "public_sha256": "...",
  "secret_profile_sha256": "...",
  "secret_count": 1,
  "reproducibility": "secret-dependent-versioned"
}
~~~

`test_plan_sha256` es el SHA-256 con prefijo `sha256:` de los bytes internos
compactos y canónicos de `TestProjectPlan`. `inputs` se ordena por `name` y
cada nombre es único, no vacío y no contiene saltos de línea ni barras
invertidas. Toda referencia `source.input` del plan de proyecto debe tener
exactamente un descriptor; no se aceptan descriptores huérfanos o duplicados.

Un input `public` debe proporcionar `sha256` válido y no puede proporcionar
`provider`, `descriptor` ni `version`. Ese hash es la identidad pública del
contenido y participa en `public_sha256`, calculado sobre la lista canónica de
`(name, source, profile, sha256, capability)`. Un input `secret` debe dejar
`sha256` en `null` y proporcionar `provider` y `descriptor` no vacíos; `version`
es opcional. La descripción secreta participa en
`secret_profile_sha256`, calculado sobre
`(name, source, profile, provider, descriptor, version, capability)`, pero el
valor nunca aparece en el plan ni en ningún hash derivado de contenido. Una
capability, si se declara, debe pertenecer al registro y estar habilitada por
el target del plan de test.

`secret_count` debe coincidir con el número de inputs secretos. La
reproducibilidad es `closed` cuando no hay secretos,
`secret-dependent-versioned` cuando todos los secretos tienen versión y
`secret-dependent-unversioned` en cualquier otro caso. Los dos digests
secundarios se expresan como 64 dígitos hexadecimales sin el prefijo
`sha256:`; el digest secreto es `null` cuando no existen secretos.

`TestInputPlan::parse` rechaza campos desconocidos, hashes inválidos, deriva
del plan, capability no habilitada, mezcla de metadatos públicos y secretos,
conteos/digests incorrectos y estados de reproducibilidad falsos.
`canonical_bytes()` vuelve a emitir únicamente esta forma normalizada. No
contiene valores de inputs, secretos, paths de host, CODEOWNERS ni resultados.
La materialización, revocación y aislamiento de valores pertenecen a
`UTEST-INPUTS-001` y deben ocurrir exclusivamente dentro del worker.

## 5. Lockfile

### 5.1 Forma completa

~~~json
{
  "format": "tondo-lock-draft",
  "manifest_hash": "sha256:...",
  "standard": {
    "package_id": "toolchain:std:draft",
    "content_hash": "sha256:..."
  },
  "meta_standard": {
    "package_id": "toolchain:std-meta:draft",
    "content_hash": "sha256:..."
  },
  "packages": [
    {
      "id": "workspace:app@1",
      "content_hash": "sha256:...",
      "dependencies": [],
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
    }
  ],
  "meta_packages": [],
  "generator_inputs": [
    {
      "name": "schema",
      "sha256": "sha256:..."
    }
  ],
  "generators": [
    {
      "id": "models",
      "owner_package": "workspace:app@1",
      "provider_package": "toolchain:std-meta:draft",
      "entry": "schema.generateModels",
      "meta_model": "tondo-meta-model-0.1/1",
      "provider_hash": "sha256:...",
      "inputs": ["schema"],
      "model_roots": [],
      "outputs": [
        {
          "logical_path": "generated/models.to",
          "module": "models"
        }
      ],
      "limits": {
        "steps": 10000000,
        "memory_bytes": 67108864,
        "output_bytes": 8388608
      }
    }
  ],
  "derive_providers": [
    {
      "origin": "standard",
      "trait_package": "toolchain:std:draft",
      "trait_module": "serialization",
      "trait_name": "Serialize",
      "provider_package": "toolchain:std-meta:draft",
      "entry": "serialization.deriveSerialize",
      "meta_model": "tondo-meta-model-0.1/1",
      "provider_hash": "sha256:...",
      "limits": {
        "steps": 1000000,
        "memory_bytes": 16777216,
        "output_bytes": 1048576
      }
    },
    {
      "origin": "standard",
      "trait_package": "toolchain:std:draft",
      "trait_module": "serialization",
      "trait_name": "Deserialize",
      "provider_package": "toolchain:std-meta:draft",
      "entry": "serialization.deriveDeserialize",
      "meta_model": "tondo-meta-model-0.1/1",
      "provider_hash": "sha256:...",
      "limits": {
        "steps": 1000000,
        "memory_bytes": 16777216,
        "output_bytes": 1048576
      }
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

Cada entrada no vacía de `meta_packages` contiene `id`, `content_hash`,
`dependencies` y `sources`. Una dependency conserva `alias` y `package`; una
source conserva `physical_path`, `logical_path`, `module` y `sha256`. Las listas
usan el mismo orden canónico que su equivalente runtime, pero no contienen
`source_set` ni `interface`.

### 5.2 Correspondencia exacta

`manifest_hash` es SHA-256 de los bytes exactos del manifiesto. Los conjuntos de
paquetes runtime y meta, fuentes, dependencies, generator inputs, generators,
derive providers y unidades privilegiadas del lockfile deben coincidir
exactamente con el plan expandido:

- mismo `PackageId`;
- mismo alias y destino exacto;
- mismo source set, path físico, path lógico y módulo;
- mismo nombre de entrada;
- mismo owner, provider, entry point, modelo meta, inputs, roots, outputs y
  límites;
- ningún elemento adicional ni ausente.

`meta_standard` debe ser exactamente el companion declarado por el descriptor de
`standard`. Los `derive_providers` con `origin: "standard"` deben coincidir con
ese mismo descriptor; los de `origin: "manifest"` deben coincidir con la lista
del proyecto. Un origen distinto, una repetición entre orígenes o una entrada
alterada se rechaza antes de lexear.

La raíz runtime no consume su propia interfaz. Todo paquete runtime no raíz y no
estándar debe tener una interfaz fijada. Los `meta_packages` se compilan desde
las fuentes fijadas en este build y no aceptan una interfaz sustitutiva; así, el
programa que produce `provider_hash` siempre forma parte de la entrada cerrada.
El hash de una interfaz runtime participa además en el hash de contenido de su
paquete.

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

Un meta package utiliza el mismo algoritmo con un record separado
`{package_id, dependencies, sources}`; sus sources omiten `source_set` y
conservan `physical_path`, `logical_path`, `module`, `sha256`. Runtime y meta no
comparten un hash aunque sus bytes fuente coincidieran.

Los paquetes estándar runtime y meta utilizan los fingerprints fijados por la
distribución actual del draft. Un hash distinto en cualquiera se rechaza.

`provider_hash` identifica el programa meta exacto que se ejecutará. Para un
provider compilado desde un paquete del grafo es el hash canónico de su artefacto
meta; para uno suministrado por la distribución es el fingerprint fijado de ese
componente. El lockfile no registra outputs generados: sus hashes se calculan
durante la ejecución y se conservan en interfaz y artefacto.

## 6. Resolución cerrada

### 6.1 Plan puro

La planificación pura sigue dos pasos:

1. `ProjectPlan::parse(manifest, lockfile)` valida identidades, condiciones,
   providers, generators, los grafos runtime/meta y la expansión del descriptor
   estándar, y devuelve la lista exacta de entradas requeridas.
2. `ProjectPlan::resolve(supplied)` acepta un mapa path → bytes y construye un
   `ResolvedProject`.

Cada entrada requerida tiene kind y SHA-256:

| Kind | Contenido |
|---|---|
| `source` | fuente de un source set activo |
| `dependency-interface` | interfaz compilada fijada |
| `meta-source` | fuente de un meta package alcanzable |
| `generator-input` | dato declarado entregado por valor al VM meta |
| `privileged-unit` | descriptor privilegiado canónico |

Falta, sobra o cambia un byte de una entrada y la resolución falla. Ninguna
entrada inactiva o meta package inalcanzable se solicita. Los programas meta se
obtienen exclusivamente del grafo `meta_packages` o del companion estándar
fijado; no son paths ambientales adicionales.

Tras validar todos los hashes, el plan construye determinísticamente
`SourceDatabase`, `PackageGraph`, `GenerationPlan` y la parte inicial de
`CompilationRequest`.

### 6.2 Separación entre frontend y orquestador

El frontend del compilador continúa siendo puro: recibe una base de fuentes
completa y nunca ejecuta código de usuario. El orquestador del toolchain controla
la fase meta, ejecuta programas en el VM cerrado descrito abajo y solo después
entrega al frontend las fuentes escritas y generadas fusionadas.

Así, “generación durante compilación” describe una fase del build, no una
capacidad escondida en imports, parsing, resolución o evaluación constante. Un
cliente que ya posea outputs válidos y su identidad completa puede invocar el
frontend sin incluir un VM.

### 6.3 Target `tondo-meta`

Providers y generators son paquetes Tondo ordinarios compilados primero para:

~~~text
target       = tondo-meta
profile      = meta
capabilities = []
~~~

Un paquete meta no puede contener `derive`, declarar otro generador ni depender
de una interfaz producida por generación en el mismo build. Sus entry points
tienen exactamente una de estas firmas lógicas:

~~~tondo pseudocode
fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error
fn expandDerive(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error
~~~

`GenerateRequest` contiene la clausura pública de `model_roots` codificada como
`tondo-meta-model-0.1/1`, los inputs declarados por nombre, la lista cerrada de
outputs y los límites. Una lista de roots vacía no incluye declaraciones.
`DeriveRequest` contiene los roots implícitos del trait y target, la declaración
`derive` y la vista privada limitada del único target autorizado. Los tipos son
opacos fuera de `std.meta`; no son una API runtime de aplicación.

`GenerateResponse` contiene exactamente un mapa `logical_path → UTF-8 source` y
diagnósticos estructurados. `DeriveResponse` contiene una única expansión
`impl`, diagnostics y su source map; el toolchain le asigna el path reservado de
2.2. Ninguna respuesta expone stdout como resultado, mutaciones del AST ni
handles hacia el compilador.

Los offsets de un source map se refieren a los bytes UTF-8 de la respuesta
original. El formatter devuelve el mapping de edits y el toolchain lo compone
con el mapa del provider para obtener spans finales. Ranges inválidos,
solapamientos no admitidos o una asociación que no sobreviva a esa composición
producen `E2105`; nunca se atribuye silenciosamente el span al archivo completo.

### 6.4 Sandbox y presupuestos

El VM meta ofrece asignación administrada y operaciones puras de `std.meta`.
Rechaza filesystem, red, procesos, environment, reloj, entropy, threads, FFI,
`unsafe`, `Pointer` y suspensión. Un generator input se lee desde el request; su path
físico nunca se revela.

Los contadores de pasos, memoria viva y bytes de salida se comprueban de forma
determinista. Agotar uno produce `E2107`; intentar una capacidad ausente produce
`E2108`. Un pánico se convierte en un diagnóstico de generación. Cada run parte
de un VM, heap y estado meta nuevos; no existe storage persistente ni estado
compartido entre producers.

### 6.5 Una sola ronda

El orquestador ejecuta:

1. resolución cerrada de todos los bytes;
2. compilación y validación de paquetes meta;
3. resolución preliminar de las clausuras escritas seleccionadas por
   `model_roots` y por cada `derive`, rechazando con `E2109` cualquier
   dependencia hacia una salida de la ronda;
4. construcción de un snapshot canónico por request y ejecución independiente
   de providers y generators contra él;
5. validación exacta de outputs, formato canónico y fusión atómica; y
6. compilación completa con la base de fuentes resultante.

Todos los snapshots se derivan de la misma base pre-generación; cada producer
recibe únicamente la clausura de sus roots y ninguno observa outputs de otro. La
fuente generada no puede contener `derive` ni solicitar otra ronda. Un path
ausente, adicional o colisionante produce `E2106`; fuente inválida o una
expansión fuera de su autorización produce `E2105`.

El orden operativo no es observable. El toolchain puede ejecutar producers en
paralelo, pero ordena solicitudes, outputs y diagnósticos por sus identidades
canónicas antes de fusionarlos.

### 6.6 Identidad y cache

`GenerationPlan` calcula una identidad por producer a partir de:

- compilador y VM meta exactos;
- edición, target, perfil, capabilities y features del consumidor;
- versión, roots y hash canónico del snapshot meta;
- PackageId, `provider_hash` y entry point;
- hashes de inputs;
- lista de outputs; y
- los tres presupuestos.

Una cache local o remota futura solo puede devolver una respuesta cuyo hash y
manifest de outputs coincidan con esa identidad. Cada salida aceptada se
formatea primero y su hash se calcula sobre esos bytes finales. El artefacto
registra inputs, producers y outputs; cambiar cualquiera invalida el build.

Si cualquier producer falla, no se publica output parcial, interfaz ni artefacto.

## 7. Interfaces compiladas

### 7.1 Propósito

Una interfaz identifica la superficie Tondo pública de un paquete y las
condiciones exactas bajo las que se comprobó. No es bytecode, no contiene
layout, no fija name mangling y no promete una ABI.

### 7.2 Schema canónico

~~~json
{
  "format": "tondo-interface-draft",
  "compiler": "tondo-bootstrap/draft",
  "edition": "0.1",
  "package_id": "registry:util@2#sha256-content",
  "target": "tondo-vm-hosted",
  "profile": "hosted",
  "capability_registry": "tondo-capabilities-draft",
  "capabilities": ["console", "process"],
  "features": ["fast"],
  "meta_model": "tondo-meta-model-0.1/1",
  "source_sets": ["@30:registry:util@2#sha256-content#common"],
  "modules": ["util"],
  "generation_hash": "sha256:...",
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

`meta_model` es `null` si el paquete no contiene fuente generada ni expansiones
`derive`. En otro caso fija la versión exacta utilizada. `generation_hash` es
SHA-256 de la secuencia canónica de identidades de producer y hashes de outputs
que contribuyen al paquete; para un paquete sin generación es el hash de la
secuencia vacía.

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
- modelo meta y generation hash;
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
  "format": "tondo-artifact-draft",
  "compiler": "tondo-bootstrap/draft",
  "edition": "0.1",
  "source_form": "module",
  "package_id": "workspace:app@1",
  "target": "tondo-vm-hosted",
  "profile": "hosted",
  "capability_registry": "tondo-capabilities-draft",
  "capabilities": ["console", "process"],
  "features": ["fast"],
  "meta_model": "tondo-meta-model-0.1/1",
  "source_sets": [
    "@15:workspace:app@1#common"
  ],
  "manifest_hash": "sha256:...",
  "lockfile_hash": "sha256:...",
  "generator_inputs": {
    "privileged:vendor.native": "sha256:...",
    "schema": "sha256:..."
  },
  "generation": [
    {
      "kind": "generator",
      "id": "models",
      "provider_package": "toolchain:std-meta:draft",
      "provider_hash": "sha256:...",
      "entry": "schema.generateModels",
      "model_roots": [],
      "model_hash": "sha256:...",
      "request_hash": "sha256:...",
      "outputs": [
        {
          "source_id": "gen:...",
          "module": "models",
          "path": "generated/models.to",
          "sha256": "sha256:..."
        }
      ]
    }
  ],
  "source_hashes": [
    {
      "source_id": "gen:...",
      "module": "models",
      "path": "generated/models.to",
      "sha256": "sha256:..."
    },
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
de la raíz. `source_hashes` se ordena por `(source_id, module, path)` e incluye
fuente escrita y generada. `generator_inputs` incluye las entradas ordinarias por
nombre y cada unidad privilegiada como `privileged:<id>`.

`generation` se ordena por `(kind, id)`. `kind` es `derive` o `generator`;
`id` es el ID del manifiesto para un generator y el `source_id`
`derive:<64hex>` para una expansión. `model_roots` conserva los roots explícitos
o implícitos del request y `model_hash` fija su clausura canónica. `outputs` se
ordena por `(source_id, module, path)`. `request_hash` cubre todos los
componentes de 6.6, no solo los inputs crudos. Un build sin generación utiliza
`meta_model: null` y `generation: []`.

`build_hash` es SHA-256 de una codificación canónica que contiene, en orden:
compilador, edición, source form, PackageId raíz, target, perfil, capacidades,
features, modelo meta, source sets, hashes de manifiesto y lockfile, generator
inputs, generation, source hashes e interface hash.

`reproducible: true` solo es válido porque todas esas entradas son explícitas,
el VM meta está cerrado y el compilador puro no accede a estado ambiental.

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
  "format": "tondo-privileged-unit-draft",
  "id": "vendor.native",
  "provider": "registry:vendor-native@1#sha256-content",
  "compiler": "tondo-bootstrap/draft",
  "target": "tondo-vm-hosted",
  "profile": "hosted",
  "capability_registry": "tondo-capabilities-draft",
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

## 10. CLI de proyecto

Las formas normales de proyecto son:

~~~text
tondo check [--project <dir>]
tondo build [--project <dir>] [--output <path>]
tondo run [--project <dir>] -- [argument ...]
tondo test [--project <dir>] [--test-plan <tondo.test.toml>] [opciones de test]
~~~

Sin `--project` se usa el directorio actual. `check`, `build` y `run`
materializan el grafo convencional descrito en la sección 3. `test` aplica las
mismas reglas y además descubre las fuentes de test según
`TONDO_TESTING_SPEC.md`. No existe un modo de proyecto basado en manifiestos
JSON.

Opciones:

~~~text
--emit-interface <path>  interfaz canónica de la raíz
--emit-artifact <path>   metadatos canónicos del build
~~~

La forma convencional obtiene `tondo.lock.toml` de la raíz y materializa los
registros internos sin escribirlos. `fmt` sigue operando sobre un único `.to` y
no acepta proyectos ni productos.

### 10.1 Build nativo

`tondo build` utiliza el target, perfil, capabilities, source sets y backend
fijados por el plan cerrado. No existe un flag que cambie de backend o target
sin cambiar esa identidad. En el bootstrap, un target que no publique un
producto ejecutable devuelve un error de tooling; nunca escribe bytecode
interno fingiendo que es un binario nativo.

`--output` elige únicamente el path físico de publicación. No participa en el
hash semántico ni permite seleccionar formato, ABI o linker. Sin `--output`, el
target descriptor determina un path bajo `target/` a partir del nombre lógico
del paquete y de las convenciones del target. El producto se construye en un
path temporal sibling, se valida y se publica mediante reemplazo atómico solo
después de completar frontend, generación, lowering, código nativo y enlace.
Un fallo conserva cualquier producto anterior y elimina el temporal.

El compilador puro produce un plan de enlace cerrado; no ejecuta el linker. El
orquestador puede ejecutar únicamente el driver interno elegido por el target,
con argumentos estructurados sin shell y con objetos, runtime, stdlib, unidades
privilegiadas y metadata fijados por hash. La identidad exacta del driver y sus
inputs forma parte del toolchain/target. No se admite búsqueda por `PATH`, flags
ambientales ni un linker configurable por el proyecto. Este contrato permite
un ejecutable Tondo autocontenido sin prometer ABI FFI, object format público,
dynamic linking ni librerías enlazables por terceros.

`tondo run` conserva una única forma: compila y ejecuta el target seleccionado
por el plan. Mientras el target sea `tondo-vm-hosted` usa la VM; un target nativo
conforme ejecuta el mismo producto que `tondo build` habría publicado, desde un
path temporal privado, y conserva stdout, stderr, argumentos y exit status. No
existe `--native`, `--vm` ni una segunda semántica de ejecución.

#### 10.1.1 Descriptor de target nativo

El descriptor nativo usa el formato interno
`tondo-native-target-descriptor-draft`. Su shape, campos y negativos ejecutables
están registrados en `testing/native-target-descriptor.json` y su contrato
detallado está en `docs/contracts/native-target-descriptor.md`.

El record canónico contiene exactamente la identidad del backend
(`name`, `version`, `implementation_hash`), la identidad lógica del target
(`name`, triple canónico en minúsculas y `profile`), `object_format`,
`runtime_abi`, el registro de capabilities, listas de capabilities/features y
flags deterministas, las referencias ordenadas de `driver` y `linker`, y la
lista de artefactos de toolchain con sus hashes. Las listas con semántica de
conjunto son ordenadas y sin duplicados; el orden de los argumentos de driver y
linker se conserva.

El lector rechaza campos desconocidos, bytes no canónicos, triples no
canónicos, formatos de objeto fuera de `elf`/`macho`/`coff`, hashes inválidos,
referencias ausentes o de tipo incorrecto y cualquier identidad o argumento
que contenga un path físico o una expansión de `$`, `%` o backtick. Cada
selección resuelve únicamente artefactos declarados por identidad y SHA-256:
no consulta `PATH`, variables de entorno, shell ni flags ambientales. La
identidad de contenido del descriptor es el SHA-256 de sus bytes canónicos.

Este descriptor fija inputs para `NATIVE-ARTIFACT-001` y
`NATIVE-LINK-PLAN-001`; no selecciona todavía un backend de producción, no
ejecuta código nativo y no promete ABI FFI, layout de objetos, dynamic linking
ni name mangling públicos.

#### 10.1.2 Clausura de artefactos nativos

`NATIVE-ARTIFACT-001` extiende la clausura semántica del artifact draft con el
record especializado `tondo-native-artifact-draft`. El artifact ordinario
(`tondo-artifact-draft`) sigue describiendo fuentes, generación e interfaz del
programa Tondo; el record nativo enlaza esos bytes con el producto que más
adelante consumirá el plan de enlace.

Su forma canónica compacta tiene exactamente estos campos, en este orden:

~~~json
{
  "format": "tondo-native-artifact-draft",
  "compiler": "tondo-bootstrap/draft",
  "edition": "0.1",
  "package_id": "workspace:app@1",
  "target_descriptor_hash": "sha256:...",
  "source_artifact_hash": "sha256:...",
  "nodes": [
    {"id":"object-main", "kind":"object", "role":"input",
     "sha256":"sha256:...", "producer":null},
    {"id":"runtime", "kind":"runtime", "role":"input",
     "sha256":"sha256:...", "producer":null},
    {"id":"stdlib", "kind":"stdlib", "role":"input",
     "sha256":"sha256:...", "producer":null},
    {"id":"product", "kind":"product", "role":"output",
     "sha256":"sha256:...", "producer":"link"}
  ],
  "producers": [
    {"id":"link", "kind":"link", "inputs":["object-main","runtime","stdlib"],
     "outputs":["product"], "sha256":"sha256:..."}
  ],
  "product_id": "product",
  "artifact_hash": "sha256:...",
  "reproducible": true
}
~~~

`nodes` y `producers` son listas ordenadas por `id`; `inputs` y `outputs` son
conjuntos ordenados. Los únicos kinds de nodo son `object`, `runtime`,
`stdlib`, `privileged-unit` y `product`. Un input no tiene producer; un
intermediate es un object producido por `compile` o `prepare`; el único output
es `product`, producido por un único producer `link`. El grafo completo debe
ser alcanzable desde `product_id` y no puede contener ciclos. Debe declarar al
menos un object input, exactamente un runtime y exactamente un stdlib; las
unidades privilegiadas son opcionales y pueden ser varias.

`target_descriptor_hash` fija el descriptor completo y
`source_artifact_hash` fija el artifact Tondo de origen. Cada node y producer
lleva un SHA-256 validado; el lector puro no abre paths para recalcularlo. La
identidad semántica `artifact_hash` se recalcula sobre todos los campos salvo
ella misma, mientras que `content_hash` es el hash de los bytes canónicos del
record. `reproducible` solo puede ser `true`.

No se serializan paths físicos, comandos, símbolos, layout de objetos,
calling convention ni ABI FFI. El record solo prueba qué inputs inmutables y
qué transformaciones cerradas forman el producto; `NATIVE-LINK-PLAN-001` fija
el orden de enlace y `NATIVE-PUBLISH-SPEC-001` fija staging y publicación.
La forma ejecutable, sus negativos y sus tests están en
`docs/contracts/native-artifact.md` y `testing/native-artifact.json`.

### 10.2 Doc-tests

La forma pública de documentación es:

~~~text
tondo doc-test --edition 0.1 <markdown>
~~~

Su scanner, fixtures, categorías, orden y schema JSON son exactamente los de la
sección 21.6 de `TONDO_LANGUAGE_SPEC.md`. El comando no descubre proyecto,
manifest ni tests, no ejecuta red o generators y habilita únicamente el perfil
`core`. Los fixtures normativos forman parte versionada del toolchain. Todo el
documento se valida antes de publicar el array JSON; un header, UTF-8, fixture o
fence inválido produce exit de diagnóstico y ningún resultado parcial.

La forma, opciones, selección, ejecución y reportes de `tondo test` se rigen por
`TONDO_TESTING_SPEC.md`; antes de invocar al compilador debe materializar el
plan cerrado descrito arriba.

### 10.3 Referencia informativa a TLF

La forma compacta para agentes y todos sus comandos, formatos, source maps,
límites y diagnósticos pertenecen exclusivamente a
`TONDO_LLM_FORM_SPEC.md` y a Gate L0. Esta referencia no incorpora TLF al
contrato de toolchain base ni al alcance de G5.

La CLI:

1. descubre el layout/TOML y genera los registros internos;
2. crea el plan puro;
3. lee exactamente `required_inputs`, relativos a la raíz del proyecto;
4. entrega los bytes al plan;
5. compila y valida los programas meta;
6. construye el snapshot, ejecuta la única ronda de generación y valida todas
   sus salidas;
7. compila la base de fuentes completa; y
8. escribe productos solo si todas las fases tienen éxito.

Un producto no puede declarar el mismo path que una fuente suelta, archivo de
configuración del proyecto, source activo o generado, interfaz de dependencia,
unidad privilegiada u otro producto de esa invocación.

## 11. Determinismo y frontera ambiental

Dos ejecuciones con bytes idénticos de:

- manifiesto y lockfile;
- fuentes activas;
- interfaces;
- generator inputs;
- programas, modelos, límites y outputs de generators y derive providers;
- unidades privilegiadas;
- misma versión de compilador;

producen bytes idénticos de interfaz y artefacto.

El orden físico de lectura no modifica source IDs, módulos, API hash, orden de
diagnósticos ni build hash. Todo set o mapa que cruza una frontera serializada
se ordena por su identidad canónica.

El módulo de planificación y el frontend del compilador no importan ni invocan
APIs de filesystem, entorno, red, proceso o reloj. El orquestador solo usa
filesystem para leer los paths exactos del plan y escribir productos solicitados;
el VM meta no lo observa. Un frontend diferente puede obtener los bytes desde
memoria, un sandbox o un content-addressed store sin cambiar la compilación.

## 12. Errores de frontera

Los errores de manifiesto, lockfile, hashes, interfaces y unidades privilegiadas
son errores del toolchain, no diagnósticos de fuente Tondo. Ocurren antes de
lexear cuando la incompatibilidad ya es visible.

Los errores de fuente conservan sus códigos normativos. En particular:

- `E1008`: módulo o API ausente por target/capacidad;
- `E1701`: llamada u operación raw fuera de una región `unsafe`;
- `E1702`: captura de `Pointer` por un cierre seguro.
- `E2101`–`E2105`: solicitud o fuente generada inválida asociada a un span.
- `E2106`–`E2108`: contrato, presupuesto o capability meta inválido; cuando no
  existe span causal usan la identidad del target y ubicación nula según 22.3.
- `E2109`: un root semántico depende de una salida de la misma ronda; usa como
  primario el root declarado o la declaración `derive` y relaciona el primer
  uso que cruza la frontera.

No se fabrica un span de fuente para un error del grafo cerrado.

## 13. Límites deliberados de 0.1

Este contrato no define:

- resolución semver o acceso a registries;
- comandos para actualizar el lockfile;
- cache remota o incremental;
- firma criptográfica de paquetes;
- formato de bytecode persistente;
- formato público de objetos, linker configurable por el usuario o dynamic
  linking; el driver nativo interno cerrado de 10.1 sí forma parte del target;
- ABI FFI general;
- compatibilidad de interfaces entre compiladores distintos.
- plugins nativos de compiler, generación multi-round o dependencias entre
  outputs generados.

Añadir cualquiera de esas piezas no puede relajar la entrada cerrada del
compilador ni convertir estado ambiental no declarado en semántica fuente.
