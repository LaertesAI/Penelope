// Review a list of files with exactly 3 agents, then check every finding with a second agent.
// Use this for the C6 measurement: 3 agents on 4 files, the first agent gets two files.
phase("review");
let files = args.files;
let n = if files.len() < 3 { files.len() } else { 3 };
if n == 0 { throw "give args.files"; }
let groups = [];
for i in 0..n { groups.push([]); }
for (i, f) in files { groups[i % n].push(f); }
let findings = parallel(groups.map(|g| #{
    prompt: "Review these files and name every real bug: " + g.join(", "),
    opts: #{label: g.join("+"), model: "weak"}
}));
let checked = findings.map(|f| agent(
    "Check this finding and answer PASS or FAIL with one reason: " + f,
    #{model: "weak", label: "check"}
));
log("review done");
checked
