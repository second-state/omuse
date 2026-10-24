"""Execution time of a task = wall time − time spent waiting for the model.

Model time is the UNION of the model-request intervals recorded by tests/model_proxy.py that overlap the task window
(parallel requests count once), plus the time the app sleeps on a Retry-After: after a 429/503 response that carried
Retry-After, the gap until the next request for the same model counts as model time too. Optionally, windows in which
the task waited for the user (approvals) are excluded from both wall and model time, so the result compares the
agent's own work between implementations.

    from tests.exec_time import execution_time
    t = execution_time(t0, t1, proxy_intervals, exclude=[(a0, a1)])
    t["exec_s"], t["model_s"], t["wall_s"], t["model_calls"], t["retries"]

All timestamps are on the monotonic clock (time.monotonic(), shared by the processes of one machine).
"""
from __future__ import annotations


def _union(segs):
    out = []
    for a, b in sorted((a, b) for a, b in segs if b > a):
        if out and a <= out[-1][1]:
            out[-1][1] = max(out[-1][1], b)
        else:
            out.append([a, b])
    return [(a, b) for a, b in out]


def _length(segs):
    return sum(b - a for a, b in segs)


def _subtract(segs, cut):
    """segs − cut, both sorted disjoint lists."""
    out = []
    for a, b in segs:
        pieces = [(a, b)]
        for c, d in cut:
            nxt = []
            for x, y in pieces:
                if d <= x or c >= y:
                    nxt.append((x, y))
                    continue
                if c > x:
                    nxt.append((x, c))
                if d < y:
                    nxt.append((d, y))
            pieces = nxt
        out += pieces
    return out


def _retry_after(v) -> float | None:
    try:
        return max(0.0, float(v))
    except (TypeError, ValueError):
        return None


def model_segments(t0: float, t1: float, intervals: list[dict], now: float | None = None) -> list[tuple[float, float]]:
    """Model-busy segments clipped to [t0, t1]: request intervals plus Retry-After sleeps."""
    end_default = t1 if now is None else min(t1, now)
    reqs = sorted((iv for iv in intervals if iv.get("start") is not None), key=lambda iv: iv["start"])
    segs = []
    for i, iv in enumerate(reqs):
        a, b = iv["start"], iv.get("end") if iv.get("end") is not None else end_default
        segs.append((max(a, t0), min(b, t1)))
        ra = _retry_after(iv.get("retry_after"))
        if iv.get("retry") and ra is not None:
            nxt = next((x["start"] for x in reqs[i + 1:] if x.get("model") == iv.get("model") and x["start"] >= b), None)
            gap_end = nxt if nxt is not None else b + ra
            segs.append((max(b, t0), min(gap_end, t1)))
    return _union(segs)


def execution_time(t0: float, t1: float, intervals: list[dict], exclude=(), now: float | None = None) -> dict:
    window = [(t0, t1)]
    cut = _union([(max(a, t0), min(b, t1)) for a, b in exclude])
    active = _subtract(window, cut)
    model = _subtract(model_segments(t0, t1, intervals, now), cut)
    inside = [iv for iv in intervals if iv.get("start") is not None and t0 <= iv["start"] <= t1]
    wall = _length(active)
    model_s = _length(model)
    return {"wall_s": round(wall, 2), "model_s": round(model_s, 2), "exec_s": round(wall - model_s, 2),
            "excluded_s": round(_length(cut), 2), "model_calls": len(inside),
            "retries": sum(1 for iv in inside if iv.get("retry"))}


if __name__ == "__main__":   # self-check
    ivs = [{"start": 1, "end": 3, "model": "m"}, {"start": 2, "end": 4, "model": "m"},
           {"start": 5, "end": 5.5, "model": "m", "status": 429, "retry": True, "retry_after": "2"},
           {"start": 7.5, "end": 8, "model": "m"}, {"start": 20, "end": 30, "model": "m"}]
    r = execution_time(0, 10, ivs, exclude=[(9, 10)])
    assert r == {"wall_s": 9, "model_s": 6.0, "exec_s": 3.0, "excluded_s": 1, "model_calls": 4, "retries": 1}, r
    print("ok", r)
