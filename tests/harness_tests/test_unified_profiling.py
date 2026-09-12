"""Unified profiling configuration and complete-duration trace composition."""
import json
from pathlib import Path
from tests.benchmarks.support.commands import GutchCli
from tests.benchmarks.support.traces import combine_labeled_traces
from tests.benchmarks.comparison.report import _read_trace_spans


def test_cli_profile_modes_have_one_switch(tmp_path):
    for mode in ('off','summary','detailed'):
        command=GutchCli(Path('gutch'),profile_mode=mode).command('snapshot','capture','--profile-out',tmp_path)
        assert command[-2:]==['--profile',mode]
        assert ('--profile-out' in command)==(mode!='off')


def test_complete_events_do_not_overlap_sequential_operations(tmp_path):
    paths=[]
    for index in range(2):
        path=tmp_path/f'{index}.json'
        path.write_text(json.dumps({'traceEvents':[{'name':'capture','ph':'X','ts':10,'dur':100,'pid':1,'tid':1,'args':{}}]}))
        paths.append(('lane',path))
    output=tmp_path/'combined.json'
    combine_labeled_traces(paths,output)
    events=[e for e in json.loads(output.read_text())['traceEvents'] if e['ph']=='X']
    assert events[1]['ts']>=events[0]['ts']+events[0]['dur']
    assert events[0]['tid'] != events[1]['tid']
    assert _read_trace_spans(paths[0][1])==[('capture',.0001,{})]
