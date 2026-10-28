import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[2]


class PairedPackages(unittest.TestCase):
    def select(self, branches, head, base='', status=2):
        workflow = (ROOT / '.github/workflows/rust.yml').read_text()
        step = workflow.split('      - id: packages-ref\n', 1)[1]
        script = textwrap.dedent(step.split('        run: |\n', 1)[1].split('      - uses:', 1)[0])
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            git = directory / 'git'
            git.write_text('#!/bin/bash\nfor pair in $PAIRS; do\n'
                           '  if [[ "${@: -1}" == "refs/heads/${pair%=*}" ]]; then\n'
                           '    echo "${pair#*=} ${@: -1}"; exit 0\n'
                           '  fi\ndone\nexit "$MISSING_STATUS"\n')
            git.chmod(0o755)
            output = directory / 'output'
            result = subprocess.run(['bash', '-euc', script], env={
                **os.environ, 'PATH': f'{directory}:{os.environ["PATH"]}',
                'BRANCH': head, 'BASE_BRANCH': base, 'GITHUB_OUTPUT': str(output),
                'PAIRS': ' '.join(f'{k}={v}' for k, v in branches.items()),
                'MISSING_STATUS': str(status),
            }, capture_output=True, text=True)
            return result.returncode, output.read_text() if output.exists() else ''

    def test_exact_partner_wins_over_base(self):
        self.assertEqual(self.select({'slice': 'abc', 'base': 'def'}, 'slice', 'base'),
                         (0, 'ref=abc\n'))

    def test_slice_uses_its_base_partner(self):
        self.assertEqual(self.select({'base': 'def'}, 'slice', 'base'), (0, 'ref=def\n'))

    def test_unpaired_vmangos_stack_uses_packages_main(self):
        self.assertEqual(self.select({'feat/vmangos-world-data': 'abc'},
                                     'vmangos/second', 'vmangos/first'), (0, 'ref=main\n'))

    def test_unpaired_branch_uses_main(self):
        self.assertEqual(self.select({}, 'unpaired'), (0, 'ref=main\n'))

    def test_remote_failure_does_not_select_main(self):
        self.assertEqual(self.select({}, 'slice', status=128), (128, ''))
