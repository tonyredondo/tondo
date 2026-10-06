"""Require exact executable UUID guide fragments and an exercised entry point."""
import argparse
from pathlib import Path
import re
import tomllib

EXAMPLES = {
    'text-and-bytes': 'textAndBytes',
    'sentinels-and-keys': 'sentinelsAndKeys',
    'external-versions': 'externalVersions',
    'names-and-encoding': 'namesAndEncoding',
    'errors-and-results': 'errorsAndResults',
    'generate-with-providers': 'generateWithProviders',
}
IMPORTS = 'import std.bytes\nimport std.console\nimport std.uuid\n\n'
ENTRY = 'fn main(): !(bytes.BytesError | uuid.UuidError) {\n' + ''.join(
    f'    {function}()?\n' for function in EXAMPLES.values()
) + '    _ = console.println("uuid-doc-ok")\n}\n'
PROJECT = {'package': {'name': 'uuid_usage'},
           'target': {'capabilities': ['civil-clock', 'console', 'entropy']}}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    raw = path.read_bytes()
    require(raw.endswith(b'\n') and b'\r' not in raw and
            all(line.rstrip() == line for line in raw.splitlines()), 'noncanonical whitespace')
    return raw.decode()


def fragments(source):
    require(source.split('// uuid-doc:', 1)[0] == IMPORTS, 'canonical example imports differ')
    rows = re.findall(r'^// uuid-doc:([a-z-]+)\n(.*?)(?=^// uuid-doc:|\Z)', source, re.M | re.S)
    require([name for name, _ in rows] == [*EXAMPLES, 'entrypoint'], 'source fragment identities differ')
    result = {name: body.rstrip('\n') + '\n' for name, body in rows}
    for name, function in EXAMPLES.items():
        require(result[name].startswith(f'fn {function}(') and
                len(re.findall(r'^fn ', result[name], re.M)) == 1, 'example function differs')
    require(result['entrypoint'] == ENTRY, 'entrypoint must execute every example and propagate errors')
    return result


def project(document):
    rows = re.findall(r'^<!-- uuid-project-capabilities -->\n~~~toml\n(.*?)^~~~$', document, re.M | re.S)
    require(len(rows) == 1 and tomllib.loads(rows[0]) == PROJECT, 'documented project capabilities differ')
    return rows[0]


def check(document, source):
    expected = fragments(source)
    rows = re.findall(r'^<!-- uuid-doc:([a-z-]+) -->\n~~~tondo\n(.*?)^~~~$', document, re.M | re.S)
    require([name for name, _ in rows] == list(expected), 'document fragment identities differ')
    require(all(body == expected[name] for name, body in rows), 'documented code differs from executable source')
    project(document)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check', 'project', 'pure-source'])
    parser.add_argument('--document', type=Path, default=Path('docs/contracts/stdlib-uuid.md'))
    parser.add_argument('--fixture', type=Path, default=Path('tests/runtime/m11-std-uuid-doc-001.to'))
    args = parser.parse_args()
    document, source = read(args.document), read(args.fixture)
    check(document, source)
    if args.action == 'project':
        print(project(document), end='')
    elif args.action == 'pure-source':
        parts = fragments(source)
        print(IMPORTS + '\n'.join(parts[name] for name in list(EXAMPLES)[:-1]) + '\n' +
              ENTRY.replace('    generateWithProviders()?\n', ''), end='')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError) as error:
        raise SystemExit('std.uuid documentation: ' + str(error))
