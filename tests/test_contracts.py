"""Static contract tests. Passing is not evidence of a working application."""
from __future__ import annotations
import copy
import sys
import unittest
from fractions import Fraction
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tools'))
from validate_spec import (TPS, MAX_TICKS, load, validate_model, validate_package,
    schema_errors, frame_ticks, duration_ticks, render_duration_ticks,
    nearest_sample, assemble_build_fixture)


class ContractTests(unittest.TestCase):
    def setUp(self):
        self.p = load('examples/multitrack.storycut.json')
        self.by_id = {c['id']: c for c in self.p['clips']}
        self.tools = {t['name']: t for t in load('contracts/mcp-tools.json')['tools']}

    def assert_invalid(self, code):
        errors = validate_model(self.p)
        self.assertTrue(any(code in e for e in errors), '\n'.join(errors))

    def test_package_schemas_and_examples(self):
        report = validate_package()
        self.assertEqual(report['tool_contracts'], 36)
        self.assertEqual(report['atomic_operation_types'], 19)

    def test_multitrack_28_seconds(self):
        self.assertFalse(validate_model(self.p))
        self.assertEqual(duration_ticks(self.p), 28 * TPS)
        self.assertEqual(render_duration_ticks(self.p) // frame_ticks(self.p), 840)

    def test_story_workflow_schema_rejects_unsafe_or_wrong_typed_inputs(self):
        for name, filename, patch in [
            ('storycut_storyboard_assemble', 'storyboard-assemble.dry-run.json', {'audio_tracks': 'music'}),
            ('storycut_storyboard_assemble', 'storyboard-assemble.dry-run.json', {'items': []}),
            ('storycut_focal_motion_apply', 'focal-motion-apply.dry-run.json', {'from_scale': 0.9}),
            ('storycut_focal_motion_apply', 'focal-motion-apply.dry-run.json', {'preset': 1}),
            ('storycut_focal_motion_apply', 'focal-motion-apply.dry-run.json', {'targets': [{'clip_id': 'clip-a', 'focus': {'x': 1.1, 'y': 0.5}}]}),
        ]:
            with self.subTest(tool=name, patch=patch):
                request = load('examples/' + filename)
                request.update(patch)
                self.assertTrue(schema_errors(request, self.tools[name]['inputSchema']))

    def test_two_images_still_19_seconds(self):
        p = load('examples/two-images.storycut.json')
        self.assertEqual(duration_ticks(p) // frame_ticks(p), 570)

    def test_multitrack_duration_is_not_sum(self):
        self.assertGreater(sum(c['duration_ticks'] for c in self.p['clips']), duration_ticks(self.p))

    def test_disabled_track_still_occupies_time(self):
        for t in self.p['tracks']:
            t['enabled'] = False
        self.assertEqual(duration_ticks(self.p), 28 * TPS)

    def test_negative_time_rejected(self):
        self.by_id['clip-a']['start_tick'] = -1
        self.assert_invalid('SCHEMA')

    def test_zero_duration_rejected(self):
        self.by_id['clip-a']['duration_ticks'] = 0
        self.assert_invalid('SCHEMA')

    def test_fractional_tick_rejected(self):
        self.by_id['clip-a']['start_tick'] = 1.5
        self.assert_invalid('SCHEMA')

    def test_misaligned_video_tick_rejected(self):
        self.by_id['clip-a']['start_tick'] = 1
        self.assert_invalid('FRAME_ALIGNMENT')

    def test_audio_one_sample_move_is_allowed(self):
        self.by_id['sfx-clip']['start_tick'] += TPS // 48000
        self.assertFalse(validate_model(self.p))

    def test_unlinked_audio_subsample_rejected(self):
        self.by_id['sfx-clip']['start_tick'] += 1
        self.assert_invalid('SAMPLE_ALIGNMENT')

    def test_missing_asset_rejected(self):
        self.by_id['clip-a']['asset_id'] = 'missing'
        self.assert_invalid('MISSING_ASSET')

    def test_missing_track_rejected(self):
        self.by_id['clip-a']['track_id'] = 'missing'
        self.assert_invalid('MISSING_TRACK')

    def test_video_on_image_track_rejected(self):
        self.by_id['broll']['track_id'] = 'i1'
        self.assert_invalid('TRACK_KIND')

    def test_audio_on_visual_track_rejected(self):
        self.by_id['sfx-clip']['track_id'] = 'v1'
        self.assert_invalid('TRACK_KIND')

    def test_image_source_in_must_be_zero(self):
        self.by_id['clip-a']['source_in_tick'] = TPS
        self.assert_invalid('IMAGE_SOURCE_IN')

    def test_wrong_stream_kind_rejected(self):
        self.by_id['video-c-audio']['stream_index'] = 0
        self.assert_invalid('STREAM_KIND')

    def test_source_overrun_rejected(self):
        self.by_id['broll']['source_in_tick'] = 18 * TPS
        self.assert_invalid('SOURCE_OVERRUN')

    def test_duplicate_clip_id_rejected(self):
        self.p['clips'].append(copy.deepcopy(self.p['clips'][0]))
        self.assert_invalid('DUPLICATE_ID')

    def test_duplicate_stream_rejected(self):
        self.p['assets'][2]['streams'].append(copy.deepcopy(self.p['assets'][2]['streams'][0]))
        self.assert_invalid('DUPLICATE_STREAM')

    def test_bad_keyframe_endpoint_rejected(self):
        self.by_id['clip-a']['motion']['keyframes'][-1]['tick'] = 10 * TPS
        self.assert_invalid('KEYFRAME_ENDPOINTS')

    def test_motion_domain_overrun_rejected(self):
        self.by_id['clip-a']['motion']['sample_offset_tick'] = TPS
        self.assert_invalid('MOTION_DOMAIN')

    def test_zero_scale_rejected(self):
        self.by_id['clip-a']['motion']['keyframes'][0]['scale'] = 0
        self.assert_invalid('SCHEMA')

    def test_more_than_two_keys_rejected_in_first_release(self):
        self.by_id['clip-a']['motion']['keyframes'].append(copy.deepcopy(self.by_id['clip-a']['motion']['keyframes'][0]))
        self.assert_invalid('SCHEMA')

    def test_same_track_overlap_requires_transition(self):
        self.p['transitions'].pop(0)
        self.assert_invalid('UNDECLARED_OVERLAP')

    def test_wrong_transition_length_rejected(self):
        self.p['transitions'][0]['duration_ticks'] = 2 * TPS
        self.assert_invalid('TRANSITION_GEOMETRY')

    def test_cross_track_transition_rejected(self):
        self.p['transitions'][0]['track_id'] = 'v2'
        self.assert_invalid('TRANSITION_TRACK')

    def test_duplicate_transition_rejected(self):
        t = copy.deepcopy(self.p['transitions'][0]);t['id'] = 'xf-copy'
        self.p['transitions'].append(t)
        self.assert_invalid('DUPLICATE_TRANSITION')

    def test_unlinked_video_policy_rejected(self):
        self.p['links'] = []
        self.assert_invalid('LINK_REQUIRED')

    def test_av_source_offset_mismatch_rejected(self):
        self.by_id['video-c-audio']['source_in_tick'] += TPS
        self.assert_invalid('LINK_SYNC')

    def test_av_timeline_offset_mismatch_rejected(self):
        self.by_id['video-c-audio']['start_tick'] += TPS
        self.assert_invalid('LINK_SYNC')

    def test_multiple_sync_groups_rejected(self):
        link = copy.deepcopy(self.p['links'][0]);link['id'] = 'link-copy'
        self.p['links'].append(link)
        self.assert_invalid('LINK_MULTIPLE')

    def test_av_link_to_muted_video_policy_rejected(self):
        self.by_id['clip-c']['audio_policy'] = 'muted'
        self.assert_invalid('LINK_POLICY')

    def test_fades_cannot_exceed_original_domain(self):
        self.by_id['bgm']['audio']['fade_in_ticks'] = 28 * TPS
        self.assert_invalid('FADE_BOUNDS')

    def test_audio_domain_overrun_rejected(self):
        self.by_id['voice']['audio']['sample_offset_tick'] = TPS
        self.assert_invalid('AUDIO_DOMAIN')

    def test_track_audio_flags_on_video_rejected(self):
        self.p['tracks'][0]['solo'] = True
        self.assert_invalid('TRACK_AUDIO_FIELDS')

    def test_subtitle_duration_rejected(self):
        self.p['subtitles'][0]['cues'][0]['end_tick'] = TPS
        self.assert_invalid('CUE_DURATION')

    def test_subtitle_wrong_track_rejected(self):
        self.p['subtitles'][0]['track_id'] = 'a1'
        self.assert_invalid('SUBTITLE_TRACK')

    def test_two_documents_same_track_rejected(self):
        d=copy.deepcopy(self.p['subtitles'][0]);d['id']='sub-2'
        self.p['subtitles'].append(d)
        self.assert_invalid('SUBTITLE_DOCUMENT_PER_TRACK')

    def test_tick_maximum_rejected(self):
        self.by_id['clip-a']['start_tick'] = MAX_TICKS + 1
        self.assert_invalid('SCHEMA')

    def test_end_above_timeline_limit_rejected(self):
        self.by_id['logo-clip']['start_tick'] = MAX_TICKS
        self.assert_invalid('TIMELINE_LIMIT')

    def test_batch_assembly_matches_expected_fixture(self):
        p=assemble_build_fixture(load('examples/seed-after-import.storycut.json'),load('examples/operations.json'))
        self.assertEqual(p,load('examples/after-apply.storycut.json'))
        self.assertFalse(validate_model(p))

    def test_mutation_requires_revision(self):
        request=load('examples/timeline-apply.commit.json');del request['expected_revision']
        self.assertTrue(schema_errors(request,self.tools['storycut_timeline_apply']['inputSchema']))

    def test_mutation_requires_idempotency_key(self):
        request=load('examples/timeline-apply.commit.json');del request['idempotency_key']
        self.assertTrue(schema_errors(request,self.tools['storycut_timeline_apply']['inputSchema']))

    def test_subtitle_cue_insert_contract(self):
        request=load('examples/timeline-apply.commit.json')
        cue=copy.deepcopy(self.p['subtitles'][0]['cues'][0]);cue['id']='cue-new'
        request['operations']=[{'op':'subtitle.cue.insert','document_id':'sub-doc-1','cue':cue,'edit_mode':'text_and_time_preserve_syntax','allow_lossy':False}]
        self.assertFalse(schema_errors(request,self.tools['storycut_timeline_apply']['inputSchema']))

    def test_subtitle_cue_remove_contract(self):
        request=load('examples/timeline-apply.commit.json')
        request['operations']=[{'op':'subtitle.cue.remove','document_id':'sub-doc-1','cue_id':'cue-a','allow_lossy':False}]
        self.assertFalse(schema_errors(request,self.tools['storycut_timeline_apply']['inputSchema']))

    def test_empty_transaction_rejected(self):
        request=load('examples/timeline-apply.commit.json');request['operations']=[]
        self.assertTrue(schema_errors(request,self.tools['storycut_timeline_apply']['inputSchema']))

    def test_shell_operation_rejected(self):
        request=load('examples/timeline-apply.commit.json');request['operations']=[{'op':'shell.exec','command':'echo test'}]
        self.assertTrue(schema_errors(request,self.tools['storycut_timeline_apply']['inputSchema']))

    def test_unknown_mcp_argument_rejected(self):
        request=load('examples/render-clean.json');request['shell']='echo test'
        self.assertTrue(schema_errors(request,self.tools['storycut_render_start']['inputSchema']))

    def test_structured_error_envelope(self):
        response=load('examples/revision-conflict.response.json')['result']
        self.assertTrue(response['isError'])
        self.assertFalse(schema_errors(response['structuredContent'],self.tools['storycut_timeline_apply']['outputSchema']))

    def test_false_success_with_error_rejected(self):
        data=load('examples/render-start.response.json')['result']['structuredContent']
        data['error']={'code':'RENDER_FAILED','message':'failure','retryable':False,'details':{}}
        self.assertTrue(schema_errors(data,self.tools['storycut_render_start']['outputSchema']))

    def test_all_cli_tools_mapped_once(self):
        commands=load('contracts/cli.json')['commands']
        self.assertEqual({c['tool'] for c in commands},set(self.tools))
        self.assertEqual(len(commands),len(self.tools))

    def test_ntsc_and_audio_exact_timebase(self):
        self.assertEqual(TPS * 1001 // 30000, 23543520)
        self.assertEqual(TPS // 48000,14700)
        self.assertEqual(TPS // 44100,16000)
        self.assertNotEqual((TPS * 1001 // 30000) % (TPS // 48000),0)

    def test_absolute_audio_rounding_does_not_drift(self):
        f=TPS*1001//30000
        bounds=[nearest_sample(i*f,48000) for i in range(3001)]
        self.assertEqual(sum(b-a for a,b in zip(bounds,bounds[1:])),nearest_sample(3000*f,48000))
        self.assertEqual(bounds[-1],4804800)

    def test_linked_ntsc_audio_can_fall_between_samples(self):
        p=load('examples/seed-after-import.storycut.json')
        p['canvas']['fps']={'num':30000,'den':1001}
        f=frame_ticks(p)
        p['tracks']=[t for t in self.p['tracks'] if t['id'] in ('v1','a1')]
        v=copy.deepcopy(self.by_id['clip-c']);a=copy.deepcopy(self.by_id['video-c-audio'])
        for c in (v,a):
            c['start_tick']=f;c['duration_ticks']=f*3;c['source_in_tick']=f
        v['motion']['domain_duration_ticks']=f*3
        v['motion']['keyframes'][-1]['tick']=f*2
        a['audio']['domain_duration_ticks']=f*3
        a['audio']['fade_in_ticks']=0
        p['clips']=[v,a];p['links']=copy.deepcopy(self.p['links'])
        self.assertFalse(validate_model(p))

    def test_split_motion_keeps_same_sampling_domain(self):
        # Mathematical fixture, not testing an implementation of clip.split.
        p=load('examples/two-images.storycut.json')
        original=copy.deepcopy(p['clips'][0]);left=copy.deepcopy(original);right=copy.deepcopy(original)
        left['duration_ticks']=5*TPS
        right.update(id='right',start_tick=5*TPS,duration_ticks=5*TPS)
        right['motion']['sample_offset_tick']=5*TPS
        p.update(clips=[left,right],transitions=[],subtitles=[])
        self.assertFalse(validate_model(p))
        f=frame_ticks(p)
        def smooth_sample(m,local):
            u=Fraction(m['sample_offset_tick']+local,m['domain_duration_ticks']-f)
            return 3*u*u-2*u*u*u
        for frame in range(150):
            self.assertEqual(smooth_sample(original['motion'],(150+frame)*f),smooth_sample(right['motion'],frame*f))

    def test_split_audio_envelope_keeps_original_domain(self):
        a=copy.deepcopy(self.by_id['bgm']);b=copy.deepcopy(a)
        a['duration_ticks']=14*TPS
        b.update(id='bgm-right',start_tick=14*TPS,source_in_tick=14*TPS,duration_ticks=14*TPS)
        b['audio']['sample_offset_tick']=14*TPS
        self.p['clips']=[c for c in self.p['clips'] if c['id']!='bgm']+[a,b]
        self.assertFalse(validate_model(self.p))
        self.assertEqual(b['audio']['domain_duration_ticks'],28*TPS)
        self.assertEqual(b['audio']['sample_offset_tick'],14*TPS)

    def test_unprobed_fixtures_are_not_runtime_evidence(self):
        self.assertTrue(all(a['probe_status']=='unprobed' and a['sha256'] is None for a in self.p['assets']))


if __name__=='__main__':
    unittest.main()
