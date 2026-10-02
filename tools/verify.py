"""Reproduce export, mathematical/GPU tests, deterministic PNGs and GPU timings.

Run from any directory: python tools/verify.py [--blender PATH] [--enforce-budget].
The editor smoke test briefly opens a native window; --no-editor skips that step.
All subprocess logs and machine-readable results go to artifacts/verification.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'artifacts' / 'verification'


def find_blender():
    found = shutil.which('blender')
    if found:
        return found
    candidates = sorted(Path('C:/Program Files/Blender Foundation').glob('Blender */blender.exe'))
    return str(candidates[-1]) if candidates else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--blender', default=find_blender())
    parser.add_argument('--no-editor', action='store_true')
    parser.add_argument('--enforce-budget', action='store_true', help='return failure when the measured GPU median exceeds settings.target_frame_ms')
    args = parser.parse_args()
    if not args.blender:
        parser.error('Blender was not found; supply --blender PATH')
    OUTPUT.mkdir(parents=True, exist_ok=True)
    report = {'functional_checks_passed': False, 'steps': [], 'performance': None}

    def run(name, command, cwd=ROOT):
        print(f'{name}: {subprocess.list2cmdline([str(c) for c in command])}', flush=True)
        started = time.perf_counter()
        result = subprocess.run(command, cwd=cwd, text=True, encoding='utf8', errors='replace', stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        log = OUTPUT / (name + '.log')
        log.write_text(result.stdout, encoding='utf8')
        report['steps'].append({'name': name, 'command': [str(c) for c in command], 'exit_code': result.returncode, 'seconds': time.perf_counter() - started, 'log': str(log.relative_to(ROOT))})
        if result.returncode:
            print(result.stdout[-8000:], file=sys.stderr)
            raise RuntimeError(f'{name} failed; see {log}')
        return result.stdout

    try:
        run('format', ['cargo', 'fmt', '--check'])
        run('clippy', ['cargo', 'clippy', '--all-targets', '--', '-D', 'warnings'])
        run('scene_export', [args.blender, '--background', 'test_scene.blend', '--python-exit-code', '1', '--python', 'tools/export_scene.py', '--', '--output', 'assets/test_scene/scene.json'])
        run('export_tests', [args.blender, '--background', '--python-exit-code', '1', '--python', 'tools/test_export.py'])
        run('rust_tests', ['cargo', 'test'])
        run('analysis_tool_tests', [sys.executable, '-m', 'unittest', 'discover', '-s', 'tests', '-t', '.'], ROOT / 'image_analysis_tools')
        run('gpu_tests', ['cargo', 'test', 'gpu_integration', '--', '--ignored', '--test-threads=1'])
        run('release_build', ['cargo', 'build', '--release'])
        executable = ROOT / 'target' / 'release' / ('instantradiositytest.exe' if sys.platform == 'win32' else 'instantradiositytest')
        run('asset_validation', [executable, '--validate-only'])
        for suffix in ['a', 'b']:
            run('capture_' + suffix, [executable, '--width', '320', '--height', '200', '--frames', '24', '--capture', OUTPUT / ('repeat_' + suffix + '.png')])
        # Exact assertion: protocol permits skipping exploration for this check.
        result = json.loads(run('deterministic_image', [sys.executable, '-m', 'image_analysis_tools.render_debug', '--select', 'all', '--evaluate', 'match', '--tolerance', '0', '--images', OUTPUT / 'repeat_a.png', OUTPUT / 'repeat_b.png', '--compact']))
        report['deterministic_image'] = result['result']
        if not result['result']['match']:
            raise RuntimeError('Repeated renders differ on this GPU/driver; see deterministic_image.log')
        if not args.no_editor:
            run('editor_smoke', [executable, '--editor-smoke', '--width', '960', '--height', '600', '--frames', '24', '--capture', OUTPUT / 'editor.png'])
        run('full_resolution', [executable, '--frames', '64', '--capture', OUTPUT / 'render.png', '--benchmark', OUTPUT / 'benchmark.json'])
        # Fixed scene assertion: this wall patch is lit and must contain no
        # near-black grid-boundary occlusion streaks. Reuse the diagnosed region.
        result = json.loads(run('lit_wall_image', [sys.executable, '-m', 'image_analysis_tools.render_debug', '--select', 'uv-rect', '0.1', '0.05', '0.28', '0.4', '--evaluate', 'occupancy', '--condition', 'R < 0.03 AND G < 0.03 AND B < 0.03', '--images', OUTPUT / 'render.png', '--compact']))
        report['lit_wall_image'] = result['result']
        if result['result']['count']:
            raise RuntimeError('Known lit wall contains near-black pixels; see lit_wall_image.log')
        report['performance'] = json.loads((OUTPUT / 'benchmark.json').read_text())
        report['functional_checks_passed'] = True
    except (OSError, RuntimeError, ValueError) as error:
        report['error'] = str(error)
        print(error, file=sys.stderr)
    finally:
        (OUTPUT / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf8')
    if not report['functional_checks_passed']:
        return 1
    performance = report['performance']
    print(f'Functional checks passed. GPU median: {performance["gpu_median_ms"]} ms; target met: {performance["target_met"]}.')
    print(f'Report: {OUTPUT / "report.json"}')
    return 2 if args.enforce_budget and performance['target_met'] is not True else 0


if __name__ == '__main__':
    sys.exit(main())
