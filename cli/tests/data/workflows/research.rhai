// Research several questions in parallel and write the assumptions register.
phase("research");
let notes = parallel(args.questions.map(|q| #{prompt: "Research this question and keep the source: " + q, opts: #{label: "research", model: "weak"}}));
let register = agent("Write the assumptions register from these notes: " + notes, #{model: "main", label: "register"});
register
