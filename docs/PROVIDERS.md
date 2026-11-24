# Penelope providers

The table in `cli/src/providers.rs` has 23 entries: 1 for the `claude` command and 22 for HTTP endpoints.
Every address and every variable name was read in the provider's own documentation on 2026-11-24.
No request from Penelope has reached any of the 22 endpoints. No key was set on the test PC.
The two wire formats ran against a stand-in server inside the tests and against a stand-in script on this PC.

## Commands

```bash
penelope providers                                  # the table, with "key set" or "no key" per entry
penelope provider use groq                          # main model on Groq with the example model
penelope provider use anthropic --role weak --model claude-haiku-5-5
penelope provider use alibaba-intl --workspace llm-xxx --model qwen-plus
penelope provider use llamacpp --base-url http://localhost:8080/v1 --role scout
export GROQ_API_KEY=...                             # the key lives in the environment and nowhere else
```

`penelope setup` offers the same 23 entries and a 24th answer `custom` for any other address.
The full screen interface shows the same table with the command `/providers`. `penelope doctor` prints one line per model in use.

## Kinds

| Kind | Request | Key header | Tool format |
|---|---|---|---|
| `claude-cli` | the official `claude` command as a child process | none, the command has its own login | MCP server `penelope mcp` |
| `openai` | `POST BASE/chat/completions` | `Authorization: Bearer KEY` | `tools` with `type: function`, answers carry `tool_calls` |
| `anthropic` | `POST BASE/v1/messages` | `x-api-key: KEY` and `anthropic-version: 2026-10-21` | `tools` with `input_schema`, answers carry `tool_use` blocks |

## The table

| Name | Kind | Base address | Key variable | Example model | Source, read on 2026-11-24 |
|---|---|---|---|---|---|
| `claude-cli` | claude-cli | none | none | `sonnet` | `cli/src/backend.rs` of this repository |
| `anthropic` | anthropic | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `claude-opus-5-5` | https://platform.claude.com/docs/en/api/messages/create and https://platform.claude.com/docs/en/api/versioning |
| `openai` | openai | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-6.2-astra` | https://developers.openai.com/api/reference/overview and https://developers.openai.com/api/docs/quickstart |
| `openrouter` | openai | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `openai/gpt-5.2` | https://openrouter.ai/docs/quickstart and https://openrouter.ai/docs/api/reference/overview |
| `alibaba-intl` | openai | `https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` | `DASHSCOPE_API_KEY` | `qwen4-max` | https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope and https://www.alibabacloud.com/help/en/model-studio/first-api-call-to-qwen |
| `alibaba-cn` | openai | `https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | `DASHSCOPE_API_KEY` | `qwen4-max` | https://help.aliyun.com/zh/model-studio/first-api-call-to-qwen and https://www.alibabacloud.com/help/en/model-studio/regions |
| `deepseek` | openai | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` | `deepseek-flash` | https://api-docs.deepseek.com/ |
| `mistral` | openai | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` | `mistral-large-latest` | https://docs.mistral.ai/api/endpoint/chat |
| `groq` | openai | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` | https://console.groq.com/docs/openai and https://console.groq.com/docs/quickstart |
| `together` | openai | `https://api.together.ai/v1` | `TOGETHER_API_KEY` | `MiniMaxAI/MiniMax-M3.1-Flash` | https://docs.together.ai/docs/openai-api-compatibility |
| `fireworks` | openai | `https://api.fireworks.ai/inference/v1` | `FIREWORKS_API_KEY` | `accounts/fireworks/models/glm-5p3-flash` | https://docs.fireworks.ai/getting-started/quickstart |
| `gemini` | openai | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-4-hydrogen` | https://ai.google.dev/gemini-api/docs/openai |
| `xai` | openai | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-4.7` | https://docs.x.ai/docs/guides/chat-completions |
| `cerebras` | openai | `https://api.cerebras.ai/v1` | `CEREBRAS_API_KEY` | `qwen-3.8-27b` | https://inference-docs.cerebras.ai/resources/openai |
| `moonshot` | openai | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3.1-pro` | https://platform.kimi.ai/docs/guide/start-using-kimi-api |
| `moonshot-cn` | openai | `https://api.moonshot.cn/v1` | `MOONSHOT_API_KEY` | `kimi-k3.1-pro` | https://platform.moonshot.cn/docs/guide/start-using-kimi-api |
| `zhipu` | openai | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` | https://docs.z.ai/guides/develop/openai/python |
| `zhipu-cn` | openai | `https://open.bigmodel.cn/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` | https://docs.bigmodel.cn/cn/guide/develop/openai/introduction |
| `huggingface` | openai | `https://router.huggingface.co/v1` | `HF_TOKEN` | `deepseek-ai/DeepSeek-V4.1-Flash` | https://huggingface.co/docs/inference-providers/index |
| `ollama` | openai | `http://localhost:11434/v1` | none | `qwen3:8b` | https://docs.ollama.com/api/openai-compatibility |
| `lmstudio` | openai | `http://localhost:1234/v1` | `LM_API_TOKEN`, optional | `ibm/granite-4-micro` | https://lmstudio.ai/docs/developer/openai-compat and https://lmstudio.ai/docs/developer/core/authentication |
| `llamacpp` | openai | `http://localhost:9931/v1` | `LLAMA_API_KEY`, optional | `gpt-3.5-turbo` | https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md |
| `vllm` | openai | `http://localhost:8000/v1` | `VLLM_API_KEY`, optional | `Qwen/Qwen2.5-1.5B-Instruct` | https://docs.vllm.ai/en/latest/getting_started/quickstart/ and https://docs.vllm.ai/en/latest/features/tool_calling/ |

## Notes per entry

- `alibaba-intl` and `alibaba-cn`: the address holds the workspace of the account, written as `{WorkspaceId}`. The flag
  `--workspace ID` of `penelope provider use` fills it in. The page says a key from one region is rejected in another region with status 401.
- `alibaba-cn`: the English page lists Beijing with the Singapore host. The Chinese page shows `cn-beijing` 28 times,
  and the migration notice of the English page names `cn-beijing` too. The table follows the Chinese page and the migration notice.
- `deepseek`: the address ends without `/v1`, and Penelope adds `/chat/completions` to it.
- `xai`: the documentation of xAI carries the title "Chat Completions (Legacy)" for this endpoint.
- `groq`: the page lists 4 fields that return status 400: `logprobs`, `logit_bias`, `top_logprobs` and `messages[].name`.
  Penelope sends none of these 4 fields in a request.
- `huggingface`: the page limits this endpoint to chat. A suffix `:fastest` or `:cheapest` on the model picks the provider.
- `ollama`: the page says "Ollama ignores it" about the key, so the entry names no variable.
- `llamacpp`: the README gives 9931 as the default port and `--jinja` as the flag for tool calls. The server answers
  with the model it loaded, so the model name `gpt-3.5-turbo` of the README example is a placeholder.
- `vllm`: tool calls need the server flags `--enable-auto-tool-choice` and `--tool-call-parser`.
- `anthropic`: the example model is `claude-opus-5-5`, and the same reference page lists the smaller `claude-haiku-5-5`.

## Not confirmed

- The old Alibaba addresses `https://dashscope-intl.aliyuncs.com` and `https://dashscope.aliyuncs.com` have no entry.
  The documentation names both only as addresses to migrate away from. The flag `--base-url` can still set one of the 2 old addresses.
- The token `LM_API_TOKEN` of LM Studio appears in an example for the path `/api/v1/chat`. Its use on the path
  `/v1/chat/completions` was not found on the 2 pages read.
- Tool calls were found in the documentation of 10 entries: `anthropic`, `openai`, `alibaba-intl`, `mistral`, `gemini`,
  `moonshot`, `zhipu`, `ollama`, `llamacpp` and `vllm`. For the other 12 HTTP entries no page on tool calls was read.
- DeepSeek documents a second address `https://api.deepseek.com/anthropic` in the format of the Messages API. Alibaba
  documents a path `/apps/anthropic` in the same format on its regions page. Neither has an entry, because the version header they accept was not looked up.
- Example models were copied from the pages on 2026-11-24. No request checked that an account can call them.

## How a key is handled

1. `config.json` stores the field `api_key_env` with the name of the variable, for example `GROQ_API_KEY`.
2. The key is read from the environment at the start of each request and goes into 1 header.
3. A value for `api_key_env` that is no variable name is ignored. The setup refuses such a value and stores nothing.
4. The body of every answer and every error text loses the key before any other code reads it.
5. `penelope providers` and `penelope doctor` print the variable name and "key set" or "no key".

The test `no_key_reaches_the_settings_the_transcript_or_any_stored_file` runs an endpoint that repeats the key 3 times.
After 2 turns it reads all files below `.penelope` and finds the key in none of them.

## Settings of an HTTP model

```json
{
  "main": {"kind": "anthropic", "base_url": "https://api.anthropic.com", "model": "claude-opus-5-5", "api_key_env": "ANTHROPIC_API_KEY"},
  "weak": {"kind": "openai", "base_url": "https://api.groq.com/openai/v1", "model": "llama-3.3-70b-versatile", "api_key_env": "GROQ_API_KEY"}
}
```

| Field | Kinds | Default | Meaning |
|---|---|---|---|
| `max_tokens` | anthropic, openai | 16000 for anthropic, unset for openai | upper limit of one answer |
| `api_version` | anthropic | `2026-10-21` | value of the header `anthropic-version` |
| `fallbacks` | anthropic | unset | sent as it is, with the header `anthropic-beta: server-side-fallback-2026-07-01` |
| `temperature` | openai | unset | sent only when set |
| `timeout` | anthropic, openai | 300 | seconds per request |

## Limits

- No real provider call was made, so all 22 HTTP entries are untested against their real endpoint.
- Answers arrive as whole blocks for both HTTP kinds. Text piece by piece exists for the `claude` command only.
- The `anthropic` kind sends no `temperature`, because API version `2026-10-21` answers that field with status 400.
- A refusal (`stop_reason: refusal`) ends the turn with its category. The field `fallbacks` is off by default and untested.
- The tool loop ends after 12 rounds for the main model and after 10 rounds for a scout.
- A status 429 or 5xx is tried again 2 times. The wait is the value of `retry-after`, else 2 and 6 seconds.
- MCP servers of plugins and ScreenCodec are connected for the `claude` command only.
- The history file `sessions/ID.history.json` holds the blocks of the model as they came, without the privacy filter.
  The provider needs these blocks back unchanged in the next request. Requests and tool results in that file passed the filter before.
- A session keeps its wire format: one stored as `openai` cannot go on as `anthropic` or through the `claude` command.
