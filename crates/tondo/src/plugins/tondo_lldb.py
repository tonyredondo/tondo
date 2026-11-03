"""Tondo's command for lldb, as `tondo dump plugin lldb` writes it.

`tondo [tasks | task <id> | heap | objects | object <address>]` shows the
stopped program as `tondo dump read` shows a dump: the program writes one
when lldb asks it to, with its runtime's `tondo_rt_dump_to`.
"""

import os
import shlex
import subprocess
import tempfile

import lldb

TONDO = "@TONDO@"


def tondo(debugger, command, result, _internal):
    process = debugger.GetSelectedTarget().GetProcess()
    if not process.IsValid() or process.GetState() != lldb.eStateStopped:
        result.SetError("stop a Tondo program first")
        return
    path = os.path.join(tempfile.gettempdir(), "tondo-lldb-%d.dump.json" % process.GetProcessID())
    frame = process.GetSelectedThread().GetSelectedFrame()
    options = lldb.SBExpressionOptions()
    options.SetTimeoutInMicroSeconds(10 * 1000 * 1000)
    # Where the stopped task is: the call's own frames are not its.
    pc, fp = frame.GetPC(), frame.FindRegister("fp").GetValueAsUnsigned(0)
    call = '(int)tondo_rt_dump_to("%s", %du, %du)' % (path, pc, fp)
    written = frame.EvaluateExpression(call, options)
    if not written.IsValid() or written.GetValueAsSigned(0) != 1:
        result.SetError("the program wrote no dump: is it a Tondo program? %s" % written.GetError())
        return
    shown = subprocess.run([TONDO, "dump", "read", path] + shlex.split(command), capture_output=True, text=True)
    result.AppendMessage((shown.stdout + shown.stderr).rstrip())


def __lldb_init_module(debugger, _internal):
    debugger.HandleCommand("command script add -o -f %s.tondo tondo" % __name__)
