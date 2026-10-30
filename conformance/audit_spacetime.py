import os
from pathlib import Path
import sys


def deployment_arguments(arguments, module):
    if 'publish' not in arguments and 'generate' not in arguments:
        return arguments
    result = []
    remaining = iter(arguments)
    for argument in remaining:
        if argument in ['-p', '--module-path']:
            next(remaining)
        elif not argument.startswith('--build-options='):
            result.append(argument)
    return [*result, '--bin-path', str(module)]


if __name__ == '__main__':
    executable, module, *arguments = sys.argv[1:]
    if not Path(module).is_file():
        sys.exit('audit deployment Module is missing')
    os.execv(executable, [executable, *deployment_arguments(arguments, module)])
