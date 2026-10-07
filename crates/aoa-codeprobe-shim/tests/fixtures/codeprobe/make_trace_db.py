import pathlib
import sys


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print(
            "usage: make_trace_db.py <codeprobe/src> <stream-json transcript> <trace.db>",
            file=sys.stderr,
        )
        return 2
    codeprobe_src, transcript, out = (pathlib.Path(arg) for arg in argv[1:])
    sys.path.insert(0, str(codeprobe_src))
    from codeprobe.trace.recorder import TraceRecorder

    if out.exists():
        out.unlink()
    with TraceRecorder(out, run_id="aoa-fixture-run") as recorder:
        recorded = recorder.ingest_stream(
            transcript.read_text(), config="baseline", task_id="task-001"
        )
    print(f"recorded {recorded} events into {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
