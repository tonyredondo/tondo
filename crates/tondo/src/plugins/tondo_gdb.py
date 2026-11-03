"""Tondo's command for gdb, as `tondo dump plugin gdb` writes it.

`tondo [tasks | task <id> | heap | objects | object <address>]` shows the
stopped program as `tondo dump read` shows a dump: the program writes one
when gdb asks it to, with its runtime's `tondo_rt_dump_to`.
"""

import os
import shlex
import subprocess
import tempfile

import gdb

TONDO = "@TONDO@"


class Tondo(gdb.Command):
    """Show the stopped Tondo program: tondo [tasks | task <id> | heap | objects | object <address>]."""

    def __init__(self):
        super().__init__("tondo", gdb.COMMAND_DATA)

    def invoke(self, argument, _from_tty):
        inferior = gdb.selected_inferior()
        if inferior.pid == 0:
            raise gdb.GdbError("start a Tondo program first")
        path = os.path.join(tempfile.gettempdir(), "tondo-gdb-%d.dump.json" % inferior.pid)
        # Where the stopped task is: the call's own frames are not its.
        frame = gdb.selected_frame()
        register = "x29" if "aarch64" in frame.architecture().name() else "rbp"
        pc, fp = frame.pc(), int(frame.read_register(register)) & 0xFFFFFFFFFFFFFFFF
        try:
            call = '((int (*)(const char *, unsigned long, unsigned long))tondo_rt_dump_to)("%s", %du, %du)'
            written = int(gdb.parse_and_eval(call % (path, pc, fp)))
        except gdb.error as error:
            raise gdb.GdbError("the program wrote no dump: is it a Tondo program? %s" % error)
        if written != 1:
            raise gdb.GdbError("the program could not write a dump to %s" % path)
        shown = subprocess.run([TONDO, "dump", "read", path] + shlex.split(argument), capture_output=True, text=True)
        gdb.write((shown.stdout + shown.stderr).rstrip() + "\n")


Tondo()
