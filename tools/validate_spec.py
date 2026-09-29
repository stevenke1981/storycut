#!/usr/bin/env python3
"""Offline validation of StoryCut specification fixtures, NOT a media editor.

Checks JSON Schemas and selected cross-field timeline invariants. It does not
probe files, render frames, connect MCP clients, enforce OS permissions, or test
Rust, IPC, persistence or GUI behaviour.
"""
from __future__ import annotations
import copy
import json
import math
import sys
from fractions import Fraction
from pathlib import Path
from typing import Any
try:
    from jsonschema import Draft202012Validator
except ImportError:
    raise SystemExit('Install validation dependencies: python -m pip install -r requirements-validation.txt')
ROOT = Path(__file__).resolve().parents[1]
TPS = 705_600_000
MAX_TICKS = TPS * 86_400


def load(relative: str) -> Any:
    def reject_constant(value: str) -> None:
        raise ValueError(f'Non-finite JSON number: {value}')
    return json.loads((ROOT / relative).read_text(encoding='utf-8'), parse_constant=reject_constant)


def frame_ticks(project: dict[str, Any]) -> int:
    fps = project['canvas']['fps']
    n = project['timebase'] * fps['den']
    if n % fps['num']:
        raise ValueError('UNSUPPORTED_FPS: frame does not map to an integer tick')
    return n // fps['num']


def duration_ticks(project: dict[str, Any]) -> int:
    ends = [c['start_tick'] + c['duration_ticks'] for c in project['clips']]
    ends += [d['offset_tick'] + c['end_tick'] for d in project['subtitles'] for c in d['cues']]
    return max(ends, default=0)


def render_duration_ticks(project: dict[str, Any]) -> int:
    frame = frame_ticks(project)
    return ((duration_ticks(project) + frame - 1) // frame) * frame


def nearest_sample(tick: int, sample_rate: int) -> int:
    """Exact round-to-nearest ties-to-even, without binary floating point."""
    return round(Fraction(tick * sample_rate, TPS))


def schema_errors(data: Any, schema: dict[str, Any]) -> list[str]:
    return [f'SCHEMA {list(e.absolute_path)}: {e.message}'
            for e in Draft202012Validator(schema).iter_errors(data)]


def validate_model(project: dict[str, Any]) -> list[str]:
    """Return model violations only. Absence of errors does not imply renderability."""
    errors = schema_errors(project, load('contracts/project.schema.json'))
    if errors:
        return errors
    try:
        frame = frame_ticks(project)
    except ValueError as exc:
        return [str(exc)]
    sample = TPS // project['audio_sample_rate']

    def problem(code: str, message: str) -> None:
        errors.append(f'{code}: {message}')

    def indexed(items: list[dict[str, Any]], category: str) -> dict[str, Any]:
        result = {}
        for item in items:
            if item['id'] in result:
                problem('DUPLICATE_ID', category + ': ' + item['id'])
            result[item['id']] = item
        return result

    assets = indexed(project['assets'], 'asset')
    tracks = indexed(project['tracks'], 'track')
    clips = indexed(project['clips'], 'clip')
    indexed(project['transitions'], 'transition')
    indexed(project['links'], 'link')
    indexed(project['subtitles'], 'subtitle')
    linked_ids = {cid for link in project['links'] for cid in link['clip_ids']}

    for a in assets.values():
        stream_ids = [s['index'] for s in a['streams']]
        if len(stream_ids) != len(set(stream_ids)):
            problem('DUPLICATE_STREAM', a['id'])
        if a['kind'] == 'image' and (a['duration_ticks'] is not None or a['streams']):
            problem('IMAGE_ASSET_METADATA', a['id'])
    for track in tracks.values():
        if track['kind'] != 'audio' and (track['muted'] or track['solo'] or track['gain_db'] != 0):
            problem('TRACK_AUDIO_FIELDS', track['id'])

    for c in clips.values():
        cid = c['id']
        track = tracks.get(c['track_id'])
        asset = assets.get(c['asset_id'])
        if track is None:
            problem('MISSING_TRACK', cid)
        else:
            allowed = {'video': {'video', 'image'}, 'image': {'image'}, 'audio': {'audio'}, 'subtitle': set()}
            if c['kind'] not in allowed[track['kind']]:
                problem('TRACK_KIND', cid)
        if asset is None:
            problem('MISSING_ASSET', cid)
        elif c['kind'] == 'image':
            if asset['kind'] != 'image':
                problem('ASSET_KIND', cid)
            if c['source_in_tick'] != 0:
                problem('IMAGE_SOURCE_IN', cid)
        else:
            if c['kind'] == 'video' and asset['kind'] != 'video':
                problem('ASSET_KIND', cid)
            if c['kind'] == 'audio' and asset['kind'] not in ('audio', 'video'):
                problem('ASSET_KIND', cid)
            stream = next((s for s in asset['streams'] if s['index'] == c['stream_index']), None)
            if stream is None or stream['kind'] != c['kind']:
                problem('STREAM_KIND', cid)
            if asset['duration_ticks'] is not None and c['source_in_tick'] + c['duration_ticks'] > asset['duration_ticks']:
                problem('SOURCE_OVERRUN', cid)
        if c['start_tick'] + c['duration_ticks'] > MAX_TICKS:
            problem('TIMELINE_LIMIT', cid)
        if c['kind'] in ('image', 'video'):
            if c['start_tick'] % frame or c['duration_ticks'] % frame:
                problem('FRAME_ALIGNMENT', cid)
            m = c['motion']
            domain, offset = m['domain_duration_ticks'], m['sample_offset_tick']
            if domain % frame or offset % frame or domain < frame:
                problem('MOTION_ALIGNMENT', cid)
            if offset + c['duration_ticks'] > domain:
                problem('MOTION_DOMAIN', cid)
            keyframes = m['keyframes']
            ticks = [k['tick'] for k in keyframes]
            last = domain - frame
            if domain == frame:
                if ticks != [0]:
                    problem('KEYFRAME_ENDPOINTS', cid)
            else:
                if not ticks or ticks[0] != 0 or ticks[-1] != last or any(b <= a for a, b in zip(ticks, ticks[1:])):
                    problem('KEYFRAME_ENDPOINTS', cid)
        else:
            # Linked A/V endpoints are exact video ticks and can fall between
            # audio samples (e.g. 30000/1001 fps). Quantize absolute endpoints
            # at render time; never accumulate rounded clip durations.
            if cid not in linked_ids and (c['start_tick'] % sample or c['duration_ticks'] % sample):
                problem('SAMPLE_ALIGNMENT', cid)
            a = c['audio']
            if a['sample_offset_tick'] + c['duration_ticks'] > a['domain_duration_ticks']:
                problem('AUDIO_DOMAIN', cid)
            if a['fade_in_ticks'] + a['fade_out_ticks'] > a['domain_duration_ticks']:
                problem('FADE_BOUNDS', cid)
            if a['fade_in_ticks'] % sample or a['fade_out_ticks'] % sample:
                problem('FADE_ALIGNMENT', cid)

    # Validate transition references, geometry and one-transition-per-overlap.
    pairs: dict[tuple[str, str], dict[str, Any]] = {}
    for t in project['transitions']:
        a, b = clips.get(t['from_clip_id']), clips.get(t['to_clip_id'])
        if a is None or b is None:
            problem('TRANSITION_REFERENCE', t['id'])
            continue
        key = (a['id'], b['id'])
        if key in pairs:
            problem('DUPLICATE_TRANSITION', t['id'])
        pairs[key] = t
        if a['kind'] not in ('image', 'video') or b['kind'] not in ('image', 'video') or a['track_id'] != b['track_id'] or t['track_id'] != a['track_id']:
            problem('TRANSITION_TRACK', t['id'])
        a_end = a['start_tick'] + a['duration_ticks']
        b_end = b['start_tick'] + b['duration_ticks']
        if not (a['start_tick'] < b['start_tick'] < a_end <= b_end):
            problem('TRANSITION_ORDER', t['id'])
        if t['start_tick'] != b['start_tick'] or t['duration_ticks'] != a_end - b['start_tick']:
            problem('TRANSITION_GEOMETRY', t['id'])
        if t['start_tick'] % frame or t['duration_ticks'] % frame:
            problem('TRANSITION_ALIGNMENT', t['id'])
    for track in tracks.values():
        if track['kind'] not in ('video', 'image'):
            continue
        visual = sorted([c for c in clips.values() if c['track_id'] == track['id']], key=lambda c: c['start_tick'])
        for i, a in enumerate(visual):
            for j in range(i + 1, len(visual)):
                b = visual[j]
                if b['start_tick'] >= a['start_tick'] + a['duration_ticks']:
                    break
                if j != i + 1:
                    problem('TRIPLE_OVERLAP', track['id'])
                if (a['id'], b['id']) not in pairs:
                    problem('UNDECLARED_OVERLAP', a['id'] + '/' + b['id'])
        for c in visual:
            incoming = sum(t['duration_ticks'] for t in project['transitions'] if t['to_clip_id'] == c['id'])
            outgoing = sum(t['duration_ticks'] for t in project['transitions'] if t['from_clip_id'] == c['id'])
            if incoming + outgoing > c['duration_ticks']:
                problem('TRANSITION_HANDLES', c['id'])

    memberships: dict[str, int] = {}
    for link in project['links']:
        members = [clips.get(cid) for cid in link['clip_ids']]
        for cid in link['clip_ids']:
            memberships[cid] = memberships.get(cid, 0) + 1
        if any(m is None for m in members):
            problem('LINK_REFERENCE', link['id'])
            continue
        videos = [m for m in members if m['kind'] == 'video']
        audios = [m for m in members if m['kind'] == 'audio']
        if len(videos) != 1 or len(audios) != 1:
            problem('LINK_KIND', link['id'])
            continue
        v, a = videos[0], audios[0]
        if v['audio_policy'] != 'separate_linked':
            problem('LINK_POLICY', link['id'])
        if any(v[k] != a[k] for k in ('asset_id', 'start_tick', 'duration_ticks', 'source_in_tick')):
            problem('LINK_SYNC', link['id'])
    for cid, count in memberships.items():
        if count != 1:
            problem('LINK_MULTIPLE', cid)
    for c in clips.values():
        if c['kind'] == 'video' and c['audio_policy'] == 'separate_linked' and memberships.get(c['id'], 0) != 1:
            problem('LINK_REQUIRED', c['id'])

    document_tracks = set()
    for d in project['subtitles']:
        track = tracks.get(d['track_id'])
        if track is None or track['kind'] != 'subtitle':
            problem('SUBTITLE_TRACK', d['id'])
        if d['track_id'] in document_tracks:
            problem('SUBTITLE_DOCUMENT_PER_TRACK', d['track_id'])
        document_tracks.add(d['track_id'])
        indexed(d['cues'], 'cue')
        for c in d['cues']:
            if c['end_tick'] <= c['start_tick']:
                problem('CUE_DURATION', c['id'])
            if d['offset_tick'] + c['end_tick'] > MAX_TICKS:
                problem('CUE_LIMIT', c['id'])
    return errors


def assemble_build_fixture(seed: dict[str, Any], operations: list[dict[str, Any]]) -> dict[str, Any]:
    """Only assembles the supplied create-only example; NOT a command engine."""
    result = copy.deepcopy(seed)
    destinations = {'track.add': ('tracks', 'track'), 'clip.add': ('clips', 'clip'),
                    'transition.set': ('transitions', 'transition'), 'link.create': ('links', 'link')}
    for operation in operations:
        if operation['op'] not in destinations:
            raise ValueError('Fixture assembler supports only the four create-only example operations')
        target, key = destinations[operation['op']]
        result[target].append(copy.deepcopy(operation[key]))
    result['revision'] += 1
    return result


def validate_package() -> dict[str, Any]:
    catalog = load('contracts/mcp-tools.json')['tools']
    by_name = {t['name']: t for t in catalog}
    assert len(by_name) == len(catalog), 'Duplicate tool name'
    for path in list((ROOT / 'contracts').glob('*.json')) + list((ROOT / 'examples').glob('*.json')):
        json.loads(path.read_text(encoding='utf-8'), parse_constant=lambda s: (_ for _ in ()).throw(ValueError(s)))
    for name in ('project', 'operations'):
        Draft202012Validator.check_schema(load(f'contracts/{name}.schema.json'))
    for tool in catalog:
        Draft202012Validator.check_schema(tool['inputSchema'])
        Draft202012Validator.check_schema(tool['outputSchema'])
        assert tool['inputSchema']['type'] == 'object'
        assert tool['outputSchema']['type'] == 'object'
    index = load('examples/example-index.json')
    fixture_names = ['two-images.storycut.json', 'multitrack.storycut.json', 'seed-after-import.storycut.json', 'after-apply.storycut.json']
    for file in fixture_names:
        errors = validate_model(load('examples/' + file))
        assert not errors, file + ': ' + '\n'.join(errors)
    durations = {}
    for file, expected in index['expected_durations'].items():
        p = load('examples/' + file)
        actual = duration_ticks(p)
        assert actual == expected['ticks']
        assert actual // frame_ticks(p) == expected['frames']
        durations[file] = expected
    Draft202012Validator(load('contracts/operations.schema.json')).validate(load('examples/operations.json'))
    assert assemble_build_fixture(load('examples/seed-after-import.storycut.json'), load('examples/operations.json')) == load('examples/after-apply.storycut.json')
    for file, name in index['input_examples'].items():
        Draft202012Validator(by_name[name]['inputSchema']).validate(load('examples/' + file))
    for file, name in index['output_examples'].items():
        result = load('examples/' + file)['result']
        structured = result['structuredContent']
        Draft202012Validator(by_name[name]['outputSchema']).validate(structured)
        assert result['isError'] == (not structured['ok'])
        assert json.loads(result['content'][0]['text']) == structured
    for line in (ROOT / 'examples/mcp-client-messages.jsonl').read_text(encoding='utf-8').splitlines():
        assert json.loads(line)['jsonrpc'] == '2.0'
    assert (ROOT / 'examples/captions.vtt').read_text(encoding='utf-8').startswith('WEBVTT')
    assert (ROOT / 'examples/captions.ass').read_text(encoding='utf-8').count('Dialogue:') == 2
    assert '00:00:01,000 --> 00:00:04,000' in (ROOT / 'examples/captions.srt').read_text(encoding='utf-8')
    assert (ROOT / 'skills/storycut-edit/SKILL.md').read_text(encoding='utf-8').startswith('---\n')
    return {'validation_kind': 'specification_only_not_runtime', 'tool_contracts': len(catalog),
            'atomic_operation_types': index['operation_count'], 'project_fixtures': len(fixture_names),
            'input_examples': len(index['input_examples']), 'output_examples': len(index['output_examples']),
            'durations': durations,
            'not_tested': ['Rust/GUI build', 'MCP connection', 'CLI runtime', 'media probing', 'video/audio rendering', 'Windows install', 'daemon/IPC/security/transaction runtime']}


def main() -> int:
    try:
        print(json.dumps(validate_package(), ensure_ascii=False, indent=2))
        return 0
    except Exception as exc:
        print(f'Specification validation failed: {exc}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
