# Example eval task repository

This is the setup folder of the example task in `../tasks.jsonl`. The eval runner copies it to a scratch worktree,
lets the model work there, and then runs the task's check command in that copy. A task that changes the repository
does not change this folder.

The example check expects a file `answer.txt` with the single line `hello`.
