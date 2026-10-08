"""Check exact networking guide fragments, project inputs and exercised entrypoint."""
import argparse
from pathlib import Path
import re
import tomllib

EXAMPLES = {
    'values-and-keys': 'valuesAndKeys',
    'tcp-roundtrip': 'tcpRoundtrip',
    'atomic-datagrams': 'atomicDatagrams',
    'errors-and-deadlines': 'errorsAndDeadlines',
    'selection-keeps-datagrams': 'selectionKeepsDatagrams',
    'tls-configuration': 'tlsConfiguration',
}
IMPORTS = 'import std.bytes\nimport std.console\nimport std.io\nimport std.net\nimport std.time\n'
ENTRY = 'fn main(): !(bytes.BytesError | net.NetError | net.TlsError | time.ClockError) {\n' + ''.join(
    f'    {name}()?\n' for name in EXAMPLES.values()
) + '    _ = console.println("net-doc-ok")\n}\n'
PROJECT = {'package': {'name': 'net_usage'}, 'target': {
    'name': 'tondo-vm-hosted', 'profile': 'hosted',
    'capabilities': ['clock', 'console', 'network'],
    'network': {'resolver_servers': ['127.0.0.1:9']}}}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    raw = path.read_bytes()
    require(raw.endswith(b'\n') and b'\r' not in raw and
            all(line.rstrip() == line for line in raw.splitlines()), 'noncanonical whitespace')
    return raw.decode()


def fragments(source):
    require(source.split('fn ', 1)[0] == IMPORTS + '\n', 'canonical imports differ')
    rows = re.findall(r'^(fn ([A-Za-z]+)\(.*?)(?=^fn |\Z)', source, re.M | re.S)
    require([name for _, name in rows] == [*EXAMPLES.values(), 'main'], 'source functions differ')
    result = {name: body.rstrip('\n') + '\n' for body, name in rows}
    require(result['main'] == ENTRY, 'entrypoint must execute every example and propagate errors')
    return result


def check(document, source, project):
    expected = fragments(source)
    heading = '## Executable usage guide for `std.net`\n'
    require(document.count(heading) == 1, 'usage guide identity differs')
    guide = document[document.index(heading):]
    require(len(re.findall(r'^~~~tondo$', guide, re.M)) == len(EXAMPLES) + 2 and
            len(re.findall(r'^~~~toml$', guide, re.M)) == 1, 'unbound guide code block')
    imports = re.findall(r'^<!-- net-imports -->\n~~~tondo\n(.*?)^~~~$', document, re.M | re.S)
    require(imports == [IMPORTS], 'documented imports differ')
    rows = re.findall(r'^<!-- net-doc:([a-z-]+) -->\n~~~tondo\n(.*?)^~~~$', document, re.M | re.S)
    require([name for name, _ in rows] == [*EXAMPLES, 'entrypoint'], 'document fragment identities differ')
    functions = {**EXAMPLES, 'entrypoint': 'main'}
    require(all(body == expected[functions[name]] for name, body in rows), 'documented source differs')
    blocks = re.findall(r'^<!-- net-project -->\n~~~toml\n(.*?)^~~~$', document, re.M | re.S)
    require(len(blocks) == 1 and tomllib.loads(blocks[0]) == PROJECT and
            tomllib.loads(project) == PROJECT, 'explicit project and resolver inputs differ')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--document', type=Path, required=True)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--project', type=Path, required=True)
    args = parser.parse_args()
    check(read(args.document), read(args.source), read(args.project))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError) as error:
        raise SystemExit('std.net documentation: ' + str(error))
