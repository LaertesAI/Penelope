// Review a list of files with parallel agents. Every finding is checked by a second agent.
phase("review");
let files = args.files;
let findings = parallel(files.map(|f| #{prompt: "Review this file and name every real bug: " + f, opts: #{label: f, model: "weak"}}));
let checked = findings.map(|f| agent("Check this finding and answer PASS or FAIL with one reason: " + f, #{model: "weak", label: "check"}));
log("review done");
checked
