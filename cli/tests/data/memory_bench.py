#!/usr/bin/env python3
"""Measures the Penelope memory: speed, retrieval quality, availability and the privacy rule.

    python3 memory_bench.py --bin PENELOPE --dir ~/.penelope/tmp/memory-bench/run --notes 100,1000,10000,50000 \
        --sections speed,quality,avail,privacy --json result.json

Every store is synthetic and lives under --dir. The script refuses a --dir outside ~/.penelope/tmp and never reads
~/.penelope/memory. No model is called.

N notes means N waypoint note lines (5 per waypoint) and N Kerkis log entries (100 per cluster). 50 of them are
known facts; the rest is noise built from a pool of made-up words.
"""
import argparse, json, os, random, shutil, subprocess, sys, threading, time

ATTRS = [("owner", "responsible"), ("port", "socket"), ("budget", "funding"), ("deadline", "due"), ("language", "tongue"),
         ("version", "release"), ("region", "zone"), ("contact", "reach"), ("license", "terms"), ("backup", "snapshot")]
QUERY_STOP = {"what", "is", "the", "of", "which", "who", "for", "to", "a", "an", "does", "has", "about", "tell", "me", "one", "as", "its"}


def pool(rng, n):
    cons, vow = "bdfgklmnprstv", "aeiou"
    out = set()
    while len(out) < n:
        out.add("".join(rng.choice(cons) + rng.choice(vow) for _ in range(rng.choice((2, 3, 3, 4)))))
    return sorted(out)


ENV = dict(os.environ)   # main() points HOME at the bench folder: a missing --root then never falls back to the real shared memory


def now_ms():
    return time.perf_counter() * 1000


def pct(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))], 2)


def du(path):
    apparent = blocks = 0
    for base, _, files in os.walk(path):
        for f in files:
            try:
                st = os.lstat(os.path.join(base, f))
                apparent += st.st_size
                blocks += st.st_blocks * 512
            except OSError:
                pass
    return apparent, blocks


# ------------------------------------------------------------------ store

class Store:
    def __init__(self, base, n, seed=7):
        self.base, self.n, self.seed = base, n, seed
        self.root = os.path.join(base, ".penelope")
        self.shared = os.path.join(base, "global")
        self.facts, self.leaves = [], []

    def build(self):
        shutil.rmtree(self.base, ignore_errors=True)
        rng = random.Random(self.seed)
        words = pool(rng, 4000)
        mem = os.path.join(self.root, "memory")
        os.makedirs(os.path.join(mem, "notes"))
        os.makedirs(os.path.join(self.shared, "notes"))
        os.makedirs(os.path.join(self.root, "clusters"))
        json.dump({"global_memory": self.shared}, open(os.path.join(self.root, "config.json"), "w"))
        json.dump({"": {"title": "start", "signs": [], "children": []}}, open(os.path.join(self.shared, "map.json"), "w"))
        w_count = max(1, self.n // 5)
        mp = {"": {"title": "start", "signs": [], "children": []}}
        leaves = []
        for k in range(w_count):
            p, a = k // 10, k // 100
            aid = "a%d-%s" % (a, words[(a * 7) % len(words)])
            pid = "%s/p%d-%s" % (aid, p, words[(p * 13) % len(words)])
            lid = "%s/%s-%d" % (pid, rng.choice(words), k)
            for cid, parent in ((aid, ""), (pid, aid), (lid, pid)):
                if cid not in mp:
                    mp[cid] = {"title": cid.rsplit("/", 1)[-1], "signs": [], "children": []}
                    mp[parent]["children"].append(cid)
            leaves.append(lid)
        self.leaves = leaves
        lines = {lid: [] for lid in leaves}
        # 50 facts: unique entity and value tokens, filed at a random leaf and written to a log as well
        for i in range(50):
            ent, val = "zq%sx%d" % (rng.choice(words), i), "vq%sy%d" % (rng.choice(words), i)
            attr, syn = ATTRS[i % len(ATTRS)]
            leaf = rng.choice(leaves)
            lines[leaf].append("The %s of %s is %s." % (attr, ent, val))
            self.facts.append({"ent": ent, "val": val, "attr": attr, "syn": syn, "leaf": leaf})
        order = [lid for lid in leaves for _ in range(5)]
        rng.shuffle(order)
        for j in range(max(0, self.n - 50)):
            w = [rng.choice(words) for _ in range(5)]
            lines[order[j % len(order)]].append("The %s %s of %s is %s for %s." % (w[0], w[1], w[2], w[3], w[4]))
        for lid, ls in lines.items():
            signs = []
            for l in ls:
                for t in l.lower().replace(".", "").split():
                    if len(t) > 1 and t not in signs and len(signs) < 40:
                        signs.append(t)
            mp[lid]["signs"] = signs
            with open(os.path.join(mem, "notes", lid.replace("/", "__") + ".md"), "w") as f:
                f.write("# %s\n\n" % lid + "".join("- %s\n" % l for l in ls))
        json.dump(mp, open(os.path.join(mem, "map.json"), "w"), indent=2)
        # the Kerkis logs, 100 entries each; the 50 facts sit at random lines
        total = max(self.n, 50)
        per = 100
        cl = (total + per - 1) // per
        entries = [None] * total
        slots = rng.sample(range(total), 50)
        for i, s in enumerate(slots):
            f = self.facts[i]
            entries[s] = (f["ent"], f["attr"], f["val"])
        for s in range(total):
            if entries[s] is None:
                entries[s] = (rng.choice(words), rng.choice(ATTRS)[0], rng.choice(words) + " " + rng.choice(words))
        slot_of = {s: i for i, s in enumerate(slots)}
        t0 = int(time.time()) - total
        for c in range(cl):
            cid = "c%d-1700000%05d" % (c + 1, c)
            d = os.path.join(self.root, "clusters", cid)
            os.makedirs(d)
            with open(os.path.join(d, "memory.log"), "w") as f:
                for line_no in range(per):
                    s = c * per + line_no
                    if s >= total:
                        break
                    ent, attr, val = entries[s]
                    text = "\n## memory · %s\nENT: %s\nATT: %s\nVAL: %s\nSRC: mcp\nREF: docs/x.md\n" % (ent, ent, attr, val)
                    f.write(json.dumps({"t": t0 + s, "C": cid, "L": 1, "text": text}) + "\n")
                    if s in slot_of:
                        self.facts[slot_of[s]]["cluster_id"] = "%s:%d" % (cid, line_no + 1)
        return self

    def size(self):
        return du(self.base)


# ------------------------------------------------------------------ process helpers

def hwm_kb(pid):
    """Peak resident set of a live process (VmHWM). ru_maxrss would carry the RSS of this Python process over the exec."""
    try:
        for line in open("/proc/%d/status" % pid):
            if line.startswith("VmHWM:"):
                return int(line.split()[1])
    except OSError:
        pass
    return None


def run_cli(bin_, root, args, timeout=300):
    """One CLI call: dict(ms, rc, out, err, rss_kb). rss_kb is the last VmHWM seen while the child ran."""
    t = now_ms()
    p = subprocess.Popen([bin_, "--root", root] + args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV)
    out_c, err_c, peak = [], [], [0]
    th1 = threading.Thread(target=lambda: out_c.append(p.stdout.read()))
    th2 = threading.Thread(target=lambda: err_c.append(p.stderr.read()))
    th1.start(); th2.start()
    while p.poll() is None:
        v = hwm_kb(p.pid)
        if v:
            peak[0] = max(peak[0], v)
        time.sleep(0.0005)
    ms = now_ms() - t
    th1.join(); th2.join()
    return {"ms": ms, "rc": p.returncode, "rss_kb": peak[0],
            "out": b"".join(out_c).decode("utf-8", "replace"), "err": b"".join(err_c).decode("utf-8", "replace")}


class Mcp:
    def __init__(self, bin_, root, flags=(), errfile=None):
        self.err = open(errfile or os.devnull, "wb")
        self.p = subprocess.Popen([bin_, "--root", root, "mcp", "serve", "kerkis"] + list(flags), stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=self.err, env=ENV)
        self.id = 0
        self.rc = None

    def rpc(self, method, params=None):
        self.id += 1
        msg = json.dumps({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params or {}}) + "\n"
        t = now_ms()
        self.p.stdin.write(msg.encode())
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        ms = now_ms() - t
        return (json.loads(line) if line else None), ms

    def hello(self):
        return self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}})[1]

    def call(self, name, args):
        return self.rpc("tools/call", {"name": name, "arguments": args})

    @staticmethod
    def payload(r):
        try:
            text = r["result"]["content"][0]["text"]
        except Exception:
            return None
        try:
            return json.loads(text)
        except Exception:
            return text

    def close(self):
        peak = hwm_kb(self.p.pid) or 0
        try:
            self.p.stdin.close()
        except Exception:
            pass
        self.rc = self.p.wait()
        self.err.close()
        return peak


def is_ok(r):
    return r is not None and "result" in r and not r["result"].get("isError", False)


def text_of(r):
    try:
        return r["result"]["content"][0]["text"]
    except Exception:
        return str((r or {}).get("error"))


# ------------------------------------------------------------------ sections

def speed(bin_, st, reps=15):
    res = {"n": st.n}
    res["store_bytes"], res["store_disk_bytes"] = st.size()
    facts = st.facts

    def series(name, argsets):
        runs = [run_cli(bin_, st.root, a) for a in argsets]
        ms = [r["ms"] for r in runs]
        res[name] = {"first_ms": round(ms[0], 1), "p50": pct(ms[1:], 50), "p95": pct(ms[1:], 95),
                     "rss_mb": round(max(r["rss_kb"] for r in runs) / 1024, 1), "rc_bad": sum(1 for r in runs if r["rc"] != 0)}

    series("cli_find", [["memory", "find", "%s %s" % (facts[i]["attr"], facts[i]["ent"])] for i in range(reps)])
    series("cli_go_new", [["memory", "go", "%s %s" % (facts[i]["ent"], facts[i]["val"])] for i in range(reps)])
    series("cli_go_stored", [["memory", "go", "%s %s" % (facts[i]["ent"], facts[i]["val"])] for i in range(reps)])
    series("cli_note", [["memory", "note", "--path", "bench/topic%d" % (i % 5), "bench note %d about %s" % (i, facts[i]["ent"])] for i in range(reps)])
    m = Mcp(bin_, st.root, ["--allow-person"], os.path.join(st.base, "mcp.err"))
    t0 = now_ms()
    m.hello()
    out = {"spawn_and_initialize_ms": round(now_ms() - t0, 1)}
    _, t = m.call("memory_find", {"query": "%s %s" % (facts[0]["attr"], facts[0]["ent"])})
    out["find_first_ms"] = round(t, 1)

    def warm(name, calls):
        ms = [m.call(n, a)[1] for n, a in calls]
        out[name] = {"p50": pct(ms, 50), "p95": pct(ms, 95)}

    warm("find_warm", [("memory_find", {"query": "%s %s" % (facts[i % 50]["attr"], facts[i % 50]["ent"])}) for i in range(1, 51)])
    _, t = m.call("memory_search", {"query": facts[1]["ent"]})
    out["search_first_ms"] = round(t, 1)
    warm("search_warm", [("memory_search", {"query": facts[i % 50]["ent"]}) for i in range(2, 32)])
    warm("get_warm", [("memory_get", {"id": facts[i % 50]["cluster_id"]}) for i in range(50)])
    warm("add_warm", [("memory_add", {"ENT": "bench%d" % i, "ATT": "x", "VAL": "y", "level": 1, "source": "bench", "cluster": "bench-add"}) for i in range(10)])
    out["rss_mb"] = round(m.close() / 1024, 1)
    res["kerkis"] = out
    return res


def rank_of(pointers, gold):
    for i, p in enumerate(pointers):
        if p["id"] in gold:
            return i + 1
    return None


def quality(bin_, st):
    queries = []
    for i, f in enumerate(st.facts):
        tier = "A" if i % 3 == 0 else "B" if i % 3 == 1 else "C"
        if tier == "A":
            q = "what is the %s of %s" % (f["attr"], f["ent"])           # close to the stored wording
        elif tier == "B":
            q = "tell me the %s for %s" % (f["syn"], f["ent"])           # the attribute is a synonym
        else:
            q = "which one has %s as its %s" % (f["val"], f["syn"])      # asked from the value, synonym attribute
        queries.append((tier, q, {"p:" + f["leaf"], f["cluster_id"]}, f["cluster_id"]))
    m = Mcp(bin_, st.root, ["--allow-person"], os.path.join(st.base, "mcp.err"))
    m.hello()
    out = {k: {"A": [], "B": [], "C": []} for k in ("find", "search_raw", "search_keywords")}
    for tier, q, gold, cgold in queries:
        r, _ = m.call("memory_find", {"query": q, "limit": 5})
        pl = m.payload(r)
        out["find"][tier].append(rank_of(pl["pointers"], gold) if isinstance(pl, dict) else None)
        for key, text in (("search_raw", q), ("search_keywords", " ".join(w for w in q.split() if w not in QUERY_STOP))):
            r, _ = m.call("memory_search", {"query": text, "limit": 5})
            pl = m.payload(r)
            es = pl["entries"] if isinstance(pl, dict) else []
            out[key][tier].append(next((i + 1 for i, e in enumerate(es) if e["id"] == cgold), None))
    m.close()
    summary = {}
    for k, tiers in out.items():
        allr = [x for t in tiers.values() for x in t]
        summary[k] = {"@1": round(sum(1 for x in allr if x == 1) / len(allr), 2), "@5": round(sum(1 for x in allr if x is not None and x <= 5) / len(allr), 2),
                      "tiers": {t: {"@1": round(sum(1 for x in v if x == 1) / len(v), 2), "@5": round(sum(1 for x in v if x is not None) / len(v), 2)} for t, v in tiers.items()}}
    return {"n": st.n, "queries": len(queries), "recall": summary}


def count_note_lines(root, prefix=""):
    d = os.path.join(root, "memory", "notes")
    n = 0
    for f in os.listdir(d):
        if f.startswith(prefix):
            try:
                n += sum(1 for l in open(os.path.join(d, f), "rb") if l.startswith(b"- "))
            except OSError:
                pass
    return n


def map_keys(root):
    try:
        return len(json.load(open(os.path.join(root, "memory", "map.json"))))
    except Exception:
        return -1


def avail(bin_, base, n_big):
    res = {}
    # 1. 1,000 sequential calls to one server
    st = Store(os.path.join(base, "av-seq"), n_big).build()
    m = Mcp(bin_, st.root, ["--allow-person"], os.path.join(st.base, "mcp.err"))
    m.hello()
    ok = bad = 0
    lat = []
    for i in range(1000):
        f = st.facts[i % 50]
        k = i % 4
        if k == 0:
            r, t = m.call("memory_search", {"query": f["ent"]})
        elif k == 1:
            r, t = m.call("memory_get", {"id": f["cluster_id"]})
        elif k == 2:
            r, t = m.call("memory_find", {"query": "%s %s" % (f["attr"], f["ent"])})
        else:
            r, t = m.call("memory_add", {"ENT": "seq", "ATT": "i", "VAL": str(i), "level": 1, "source": "bench", "cluster": "seq-test"})
        lat.append(t)
        ok, bad = (ok + 1, bad) if is_ok(r) else (ok, bad + 1)
    rss = m.close()
    log = os.path.join(st.root, "clusters", "seq-test", "memory.log")
    res["sequential_1000"] = {"n_store": n_big, "ok": ok, "bad": bad, "p50_first100": pct(lat[:100], 50), "p50_last100": pct(lat[-100:], 50),
                              "p95": pct(lat, 95), "max_ms": round(max(lat), 1), "rss_mb": round(rss / 1024, 1), "exit": m.rc,
                              "adds_sent": 250, "adds_in_log": sum(1 for _ in open(log))}
    # 1b. the default cluster of memory_add collects everything a model adds: one log with 20,000 entries
    big = os.path.join(st.root, "clusters", "kerkis-memory")
    os.makedirs(big, exist_ok=True)
    with open(os.path.join(big, "memory.log"), "w") as f:
        for i in range(20000):
            text = "\n## memory \u00b7 e%d\nENT: e%d\nATT: a\nVAL: v%d\nSRC: mcp\nREF: x\n" % (i, i, i)
            f.write(json.dumps({"t": 1700000000 + i, "C": "kerkis-memory", "L": 1, "text": text}) + "\n")
    m = Mcp(bin_, st.root, [], os.devnull)
    m.hello()
    t_add = [m.call("memory_add", {"ENT": "n%d" % i, "ATT": "a", "VAL": "v", "level": 1, "source": "bench"})[1] for i in range(20)]
    t_get = [m.call("memory_get", {"id": "kerkis-memory:15000"})[1] for i in range(20)]
    t_search = [m.call("memory_search", {"query": "e1234 v1234"})[1] for i in range(10)]
    res["one_log_20000_entries"] = {"add_p50": pct(t_add, 50), "get_p50": pct(t_get, 50), "search_p50": pct(t_search, 50), "rss_mb": round(m.close() / 1024, 1)}

    # 2. 8 parallel clients, one server process each, same store
    results = [None] * 8

    def client(k):
        mm = Mcp(bin_, st.root, ["--allow-person"], os.path.join(st.base, "mcp%d.err" % k))
        mm.hello()
        good = fail = 0
        added = []
        for i in range(125):
            f = st.facts[(i + k) % 50]
            j = i % 5
            if j == 0:
                r, _ = mm.call("memory_search", {"query": f["ent"]})
            elif j == 1:
                r, _ = mm.call("memory_get", {"id": f["cluster_id"]})
            elif j == 2:
                r, _ = mm.call("memory_find", {"query": "%s %s" % (f["attr"], f["ent"])})
            else:
                r, _ = mm.call("memory_add", {"ENT": "par%d" % k, "ATT": "i", "VAL": str(i), "level": 1, "source": "bench", "cluster": "par-test"})
                pl = mm.payload(r) if is_ok(r) else None
                if isinstance(pl, dict):
                    added.append(pl.get("id"))
            good, fail = (good + 1, fail) if is_ok(r) else (good, fail + 1)
        mm.close()
        results[k] = (good, fail, added)

    t = now_ms()
    ths = [threading.Thread(target=client, args=(k,)) for k in range(8)]
    [x.start() for x in ths]; [x.join() for x in ths]
    wall = now_ms() - t
    ids = [i for r in results if r for i in r[2]]
    log = os.path.join(st.root, "clusters", "par-test", "memory.log")
    res["parallel_8"] = {"calls": 1000, "ok": sum(r[0] for r in results if r), "bad": sum(r[1] for r in results if r), "wall_s": round(wall / 1000, 1),
                         "adds_acked": len(ids), "adds_unique_ids": len(set(ids)), "adds_in_log": sum(1 for _ in open(log)) if os.path.exists(log) else 0}
    shutil.rmtree(st.base, ignore_errors=True)

    # 3. several processes writing notes at once
    for procs in (2, 8):
        st = Store(os.path.join(base, "av-conc"), 100).build()
        per = 25

        def writer(k):
            for i in range(per):
                run_cli(bin_, st.root, ["memory", "note", "--path", "conc/shared", "writer %d note %d sharedtext" % (k, i)])
                run_cli(bin_, st.root, ["memory", "note", "--path", "conc/own%d" % k, "writer %d own %d ownertext" % (k, i)])

        ths = [threading.Thread(target=writer, args=(k,)) for k in range(procs)]
        [x.start() for x in ths]; [x.join() for x in ths]
        try:
            mp = json.load(open(os.path.join(st.root, "memory", "map.json")))
            present = sum(1 for k in range(procs) if "conc/own%d" % k in mp) + (1 if "conc/shared" in mp else 0)
        except Exception as e:
            present = "map.json unreadable: %s" % e
        res["concurrent_notes_%dprocs" % procs] = {
            "written_shared": procs * per, "kept_shared": count_note_lines(st.root, "conc__shared"),
            "written_own": procs * per, "kept_own": sum(count_note_lines(st.root, "conc__own%d" % k) for k in range(procs)),
            "waypoints_expected": procs + 1, "waypoints_in_map": present}
        shutil.rmtree(st.base, ignore_errors=True)

    # 4. corrupt map.json
    st = Store(os.path.join(base, "av-map"), 500).build()
    mapf = os.path.join(st.root, "memory", "map.json")
    raw = open(mapf, "rb").read()
    files_before = len(os.listdir(os.path.join(st.root, "memory", "notes")))
    keys_before = map_keys(st.root)
    open(mapf, "wb").write(raw[: len(raw) // 2])
    r_go = run_cli(bin_, st.root, ["memory", "go", "rust errors"])
    r_find = run_cli(bin_, st.root, ["memory", "find", st.facts[0]["ent"]])
    r_note = run_cli(bin_, st.root, ["memory", "note", "--path", "rust/errors", "after corruption"])
    res["corrupt_map"] = {"note_files": files_before, "waypoints_before": keys_before, "waypoints_in_map_after_note": map_keys(st.root),
                          "go": {"rc": r_go["rc"], "out": r_go["out"][:90].strip()}, "find": {"rc": r_find["rc"], "out": r_find["out"][:90].strip()},
                          "note": {"rc": r_note["rc"], "out": r_note["out"][:90].strip(), "err": r_note["err"][:120].strip()},
                          "extra_files_in_memory_dir": sorted(f for f in os.listdir(os.path.join(st.root, "memory")) if f not in ("map.json", "notes", "paths.json", "journal.json", "uses.json"))}
    shutil.rmtree(st.base, ignore_errors=True)

    # 5. corrupt note files
    st = Store(os.path.join(base, "av-note"), 500).build()
    notes_dir = os.path.join(st.root, "memory", "notes")
    cases = {"non_utf8": b"# x\n\n- fine line\n- bad \xff\xfe\xfa bytes\n", "binary_garbage": os.urandom(2048), "half_written": b"# x\n\n- first line\n- half writ"}
    out = {}
    others = None
    for (name, content), leaf in zip(cases.items(), st.leaves[:3]):
        fp = os.path.join(notes_dir, leaf.replace("/", "__") + ".md")
        open(fp, "wb").write(content)
        r_go = run_cli(bin_, st.root, ["memory", "go", leaf.replace("/", " ")])
        r_find = run_cli(bin_, st.root, ["memory", "find", st.facts[0]["ent"]])
        r_note = run_cli(bin_, st.root, ["memory", "note", "--path", leaf, "after corruption %s" % name])
        after = open(fp, "rb").read()
        out[name] = {"go_rc": r_go["rc"], "find_rc": r_find["rc"], "find_still_answers": st.facts[0]["ent"] in r_find["out"] or "L3" in r_find["out"],
                     "note_rc": r_note["rc"], "note_out": r_note["out"][:50].strip(), "original_bytes_kept": content in after,
                     "bad_copy_kept": any(open(os.path.join(notes_dir, f), "rb").read() == content for f in os.listdir(notes_dir) if ".bad-" in f),
                     "file_bytes_before": len(content), "file_bytes_after": len(after)}
    res["corrupt_note_file"] = out
    shutil.rmtree(st.base, ignore_errors=True)

    # 6. missing folder
    ghost = os.path.join(base, "av-missing", "nothing", ".penelope")
    miss = {}
    r = run_cli(bin_, ghost, ["memory", "find", "anything"])
    miss["cli_find"] = {"rc": r["rc"], "out": r["out"][:100].strip(), "folder_created": os.path.isdir(ghost)}
    r = run_cli(bin_, ghost, ["memory", "go", "anything"])
    miss["cli_go"] = {"rc": r["rc"], "out": r["out"][:100].strip()}
    m = Mcp(bin_, ghost, ["--allow-person"], os.devnull)
    m.hello()
    for name, args in (("memory_search", {"query": "anything"}), ("memory_get", {"id": "nope:1"}), ("memory_find", {"query": "anything"}), ("memory_status", {})):
        r, _ = m.call(name, args)
        miss[name] = {"isError": (r or {}).get("result", {}).get("isError"), "text": text_of(r)[:100]}
    m.close()
    res["missing_folder"] = miss
    shutil.rmtree(os.path.join(base, "av-missing"), ignore_errors=True)

    # 7. very long note and long lines
    st = Store(os.path.join(base, "av-long"), 500).build()
    out = {}
    r = run_cli(bin_, st.root, ["memory", "note", "--path", "long/one", "longword " * 14000])  # 126 KB, the limit of one argument is 128 KB
    out["cli_note_126KB"] = {"rc": r["rc"], "ms": round(r["ms"]), "out": r["out"][:60].strip()}
    mp = json.load(open(os.path.join(st.root, "memory", "map.json")))
    mp["long/big"] = {"title": "big", "signs": ["longword"], "children": []}
    mp[""]["children"].append("long/big")
    json.dump(mp, open(os.path.join(st.root, "memory", "map.json"), "w"))
    fp = os.path.join(st.root, "memory", "notes", "long__big.md")
    open(fp, "w").write("# long/big\n\n- " + "bigline " * 1_250_000 + "\n- short line after the big one\n")
    out["hand_edited_line_bytes"] = os.path.getsize(fp)
    for label, args in (("find", ["memory", "find", st.facts[0]["ent"]]), ("go", ["memory", "go", "long big"])):
        r = run_cli(bin_, st.root, args)
        out["cli_%s_with_10MB_line" % label] = {"rc": r["rc"], "ms": round(r["ms"]), "rss_mb": round(r["rss_kb"] / 1024), "out_chars": len(r["out"])}
    m = Mcp(bin_, st.root, ["--allow-person"], os.devnull)
    m.hello()
    r, t = m.call("memory_add", {"ENT": "big", "ATT": "x", "VAL": "v" * 1_000_000, "level": 1, "source": "bench"})
    pl = m.payload(r)
    out["mcp_add_1MB_val"] = {"ok": is_ok(r), "ms": round(t, 1), "stored_chars": len(pl["text"]) if isinstance(pl, dict) else None}
    m.close()
    res["long_note"] = out
    shutil.rmtree(st.base, ignore_errors=True)

    # 8. non-UTF-8 in a log line and on stdin
    st = Store(os.path.join(base, "av-utf"), 500).build()
    logf = os.path.join(st.root, "clusters", sorted(os.listdir(os.path.join(st.root, "clusters")))[0], "memory.log")
    with open(logf, "ab") as f:
        f.write(b'{"t": 1, "C": "x", "L": 1, "text": "bad \xff\xfe bytes zqbadbytes"}\n')
    m = Mcp(bin_, st.root, [], os.devnull)
    m.hello()
    r, _ = m.call("memory_search", {"query": "zqbadbytes"})
    pl = m.payload(r)
    m.p.stdin.write(b'{"jsonrpc":"2.0","id":99,"method":"tools/list"\xff\xfe}\n')
    m.p.stdin.flush()
    bad_line = m.p.stdout.readline().decode("utf-8", "replace")[:120]
    r2, _ = m.call("memory_search", {"query": st.facts[0]["ent"]})
    res["non_utf8"] = {"log_line_found": pl["total"] if isinstance(pl, dict) else None, "stdin_bad_line_answer": bad_line.strip(), "server_alive_after": is_ok(r2)}
    m.close()
    shutil.rmtree(st.base, ignore_errors=True)

    # 9. read-only folders
    st = Store(os.path.join(base, "av-ro"), 500).build()
    memdir = os.path.join(st.root, "memory")
    notes_dir = os.path.join(memdir, "notes")
    before = count_note_lines(st.root)
    os.chmod(notes_dir, 0o555); os.chmod(memdir, 0o555)
    r = run_cli(bin_, st.root, ["memory", "note", "--path", "ro/test", "written into a read-only folder"])
    ro = {"note_rc": r["rc"], "note_out": r["out"][:80].strip(), "note_err": r["err"][:120].strip(), "note_persisted": count_note_lines(st.root) > before}
    cl = os.path.join(st.root, "clusters")
    cdir = os.path.join(cl, sorted(os.listdir(cl))[0])
    os.chmod(cdir, 0o555); os.chmod(cl, 0o555)
    m = Mcp(bin_, st.root, [], os.devnull)
    m.hello()
    r, _ = m.call("memory_add", {"ENT": "ro", "ATT": "x", "VAL": "y", "level": 1, "source": "bench", "cluster": os.path.basename(cdir)})
    ro["mcp_add"] = {"isError": (r or {}).get("result", {}).get("isError"), "text": text_of(r)[:120]}
    r, _ = m.call("memory_search", {"query": st.facts[0]["ent"]})
    ro["mcp_search_still_works"] = is_ok(r)
    m.close()
    for p in (cl, cdir, memdir, notes_dir):
        os.chmod(p, 0o755)
    res["read_only"] = ro
    shutil.rmtree(st.base, ignore_errors=True)
    return res


def privacy(bin_, st):
    out = {"n": st.n}
    for label, flags in (("without_flag", []), ("with_flag", ["--allow-person"])):
        m = Mcp(bin_, st.root, flags, os.devnull)
        m.hello()
        lv3 = seen = 0
        for f in st.facts[:20]:
            r, _ = m.call("memory_find", {"query": "%s %s" % (f["attr"], f["ent"]), "limit": 20})
            pl = m.payload(r)
            if isinstance(pl, dict):
                for p in pl["pointers"]:
                    seen += 1
                    lv3 += 1 if p["L"] >= 3 else 0
        r, _ = m.call("memory_add", {"ENT": "p", "ATT": "x", "VAL": "y", "level": 3, "source": "bench", "cluster": "priv-test"})
        out[label] = {"pointers_seen": seen, "level3_pointers": lv3, "add_level3_refused": not is_ok(r)}
        m.close()
    exp = {}
    for label, flags in (("without_flag", []), ("with_flag", ["--include-person"])):
        d = os.path.join(st.base, "export-" + label)
        shutil.rmtree(d, ignore_errors=True)
        r = run_cli(bin_, st.root, ["memory", "export", "--format", "json"] + flags + [os.path.join(d, "g.json")], timeout=600)
        nodes = []
        try:
            nodes = json.load(open(os.path.join(d, "g.json"))).get("nodes", [])
        except Exception:
            pass
        exp[label] = {"rc": r["rc"], "nodes": len(nodes), "level3_nodes": sum(1 for n in nodes if n.get("level", n.get("L", 0)) >= 3), "out": r["out"][:80].strip()}
    out["export"] = exp
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--dir", required=True)
    ap.add_argument("--notes", default="100,1000,10000,50000")
    ap.add_argument("--sections", default="speed,quality,avail,privacy")
    ap.add_argument("--avail-notes", type=int, default=1000)
    ap.add_argument("--json", default=None)
    a = ap.parse_args()
    base = os.path.abspath(os.path.expanduser(a.dir))
    allowed = os.path.expanduser("~/.penelope/tmp/")
    if not base.startswith(allowed):
        sys.exit("--dir must lie under %s (not /tmp, never the real memory)" % allowed)
    os.makedirs(os.path.join(base, "home"), exist_ok=True)
    ENV["HOME"] = os.path.join(base, "home")
    sections = a.sections.split(",")
    result = {"bin": a.bin, "speed": [], "quality": [], "privacy": []}
    for n in [int(x) for x in a.notes.split(",")]:
        if not {"speed", "quality", "privacy"} & set(sections):
            break
        t = now_ms()
        st = Store(os.path.join(base, "n%d" % n), n).build()
        gen = round((now_ms() - t) / 1000, 1)
        print("== N=%d store built in %.1fs" % (n, gen), flush=True)
        for name in ("quality", "privacy", "speed"):   # speed last: it writes notes into the store
            if name in sections:
                v = {"quality": quality, "privacy": privacy, "speed": speed}[name](a.bin, st)
                if name == "speed":
                    v["gen_s"] = gen
                result[name].append(v)
                print(json.dumps(v), flush=True)
        shutil.rmtree(st.base, ignore_errors=True)
    if "avail" in sections:
        result["avail"] = avail(a.bin, base, a.avail_notes)
        print(json.dumps(result["avail"], indent=1), flush=True)
    if a.json:
        json.dump(result, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
