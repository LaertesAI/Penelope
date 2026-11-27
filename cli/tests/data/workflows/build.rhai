// Build from a plan, then check the result with a second agent.
phase("build");
let plan = agent("Write a short build plan for: " + args.task, #{model: "main", label: "plan"});
let built = agent("Build this plan in the project: " + plan, #{model: "weak", label: "build", worktree: true});
let checked = agent("Run the check and review the result: " + built, #{model: "weak", label: "check"});
checked
