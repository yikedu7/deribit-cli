# Agent 组合期权查询

本文件提供可改写的 Bash + jq 样例，不安装分析程序，不增加 Rust CLI 命令。Agent 负责选择输入、调度请求、检查结果和解释数据。`jq` 不可用时可用已有工具实现相同流程。

## 先确定合约集合

用 `get-instruments` 获取元数据；API `currency` 是查询分组，不一定等于用户所说的标的。不确定时用 `--currency any --kind option --expired false`，再看 `base_currency`、`quote_currency`、`settlement_currency` 和 `instrument_name`。同一标的有多种产品时明确选择产品范围，不能默默混合结算币种。

把成功响应保存为 `instruments.json` 后，可先列出元数据中的到期时间（下例只读取文件，不请求 API）：

```sh
jq '[.result[] | {base_currency, quote_currency, settlement_currency, expiration_timestamp, expiration_utc: (.expiration_timestamp / 1000 | todateiso8601)}] | unique' instruments.json
```

确认用户指定日期对应哪个 `expiration_timestamp`；有多个匹配时先消除歧义，不四舍五入到最近一日。用于采集的参数：`DERIBIT_CURRENCY`（帮助允许的 API 分组，可为 `any`）、`DERIBIT_BASE`、`DERIBIT_QUOTE`、`DERIBIT_SETTLEMENT`、`DERIBIT_EXPIRY_MS`（精确毫秒）、`DERIBIT_SIDE`（`call`/`put`/`both`，默认 `both`）。

## 一批采集与期权链汇总

在独立 Bash 进程中运行以下片段，先设置上述参数和 `DERIBIT_BIN`。默认 mainnet，`DERIBIT_ENV=testnet` 可显式切换。每个候选 ticker 只调用一次；请求量为一次 metadata 加 N 次 ticker，先按筛选范围估算 N。收窄范围必须符合用户意图，不能为节省调用随意截断候选集。

```bash
set -euo pipefail
: "${DERIBIT_BIN:?set absolute CLI path}"
: "${DERIBIT_CURRENCY:?set API currency group}"
: "${DERIBIT_BASE:?set underlying base currency}"
: "${DERIBIT_QUOTE:?set quote currency}"
: "${DERIBIT_SETTLEMENT:?set settlement currency}"
: "${DERIBIT_EXPIRY_MS:?set exact expiration timestamp in milliseconds}"
option_side=${DERIBIT_SIDE:-both}
option_env=${DERIBIT_ENV:-mainnet}
case "$option_side" in call|put|both) ;; *) printf 'invalid option side\n' >&2; exit 2 ;; esac
case "$option_env" in mainnet|testnet) ;; *) printf 'invalid environment\n' >&2; exit 2 ;; esac
jq -en --argjson expiry "$DERIBIT_EXPIRY_MS" '$expiry | type == "number" and . > 0 and floor == .' >/dev/null
batch_dir=$(mktemp -d)
printf 'batch_dir=%s\n' "$batch_dir"
batch_started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
jq -n --arg started "$batch_started" --arg env "$option_env" --arg currency "$DERIBIT_CURRENCY" --arg base "$DERIBIT_BASE" --arg quote "$DERIBIT_QUOTE" --arg settlement "$DERIBIT_SETTLEMENT" --arg side "$option_side" --argjson expiry "$DERIBIT_EXPIRY_MS" '{environment:$env, started_at:$started, simultaneous:false, status:"collecting", filters:{currency:$currency,base_currency:$base,quote_currency:$quote,settlement_currency:$settlement,expiration_timestamp:$expiry,option_type:$side}}' > "$batch_dir/batch.json"
"$DERIBIT_BIN" version > "$batch_dir/version.json"
if "$DERIBIT_BIN" --env "$option_env" --output json public get-instruments --currency "$DERIBIT_CURRENCY" --kind option --expired false > "$batch_dir/instruments.json" 2> "$batch_dir/instruments.stderr"; then
  jq -e 'has("result") and (has("error") | not) and (.result | type == "array")' "$batch_dir/instruments.json" >/dev/null
else
  batch_rc=$?
  printf 'metadata failed: exit=%s; partial evidence=%s\n' "$batch_rc" "$batch_dir" >&2
  exit "$batch_rc"
fi
jq --arg base "$DERIBIT_BASE" --arg quote "$DERIBIT_QUOTE" --arg settlement "$DERIBIT_SETTLEMENT" --arg side "$option_side" --argjson expiry "$DERIBIT_EXPIRY_MS" '[.result[] | select(.kind == "option" and .is_active == true and .base_currency == $base and .quote_currency == $quote and .settlement_currency == $settlement and .expiration_timestamp == $expiry) | select($side == "both" or .option_type == $side)] | sort_by(.strike, .option_type, .instrument_name)' "$batch_dir/instruments.json" > "$batch_dir/candidates.json"
jq -r '.[].instrument_name' "$batch_dir/candidates.json" > "$batch_dir/names.txt"
printf 'ticker requests planned: %s\n' "$(jq 'length' "$batch_dir/candidates.json")"
mkdir "$batch_dir/tickers"
batch_i=0
while IFS= read -r instrument; do
  batch_i=$((batch_i + 1))
  if "$DERIBIT_BIN" --env "$option_env" --output json public ticker --instrument-name "$instrument" > "$batch_dir/tickers/$batch_i.json" 2> "$batch_dir/tickers/$batch_i.stderr"; then
    jq -e --arg instrument "$instrument" '(has("error") | not) and (.result | type == "object") and (.result.instrument_name == $instrument) and (.result.timestamp | type == "number")' "$batch_dir/tickers/$batch_i.json" >/dev/null
  else
    batch_rc=$?
    printf 'ticker failed: instrument=%s exit=%s; incomplete batch=%s\n' "$instrument" "$batch_rc" "$batch_dir" >&2
    exit "$batch_rc"
  fi
done < "$batch_dir/names.txt"
if [ "$batch_i" -eq 0 ]; then
  jq -n '[]' > "$batch_dir/tickers.json"
else
  jq -s '[.[].result]' "$batch_dir"/tickers/*.json > "$batch_dir/tickers.json"
fi
jq -n --slurpfile instruments "$batch_dir/candidates.json" --slurpfile tickers "$batch_dir/tickers.json" '($tickers[0] | INDEX(.instrument_name)) as $quotes | [$instruments[0][] | . as $i | ($quotes[$i.instrument_name] // {}) as $q | {instrument_name:$i.instrument_name, expiration_timestamp:$i.expiration_timestamp, strike:$i.strike, option_type:$i.option_type, base_currency:$i.base_currency, quote_currency:$i.quote_currency, settlement_currency:$i.settlement_currency, contract_size:$i.contract_size, best_bid_price:$q.best_bid_price, best_ask_price:$q.best_ask_price, mark_price:$q.mark_price, mark_iv:$q.mark_iv, greeks:$q.greeks, open_interest:$q.open_interest, volume:$q.stats.volume, timestamp:$q.timestamp}]' > "$batch_dir/chain.json"
batch_finished=$(date -u +%Y-%m-%dT%H:%M:%SZ)
jq --arg finished "$batch_finished" --argjson count "$batch_i" '. + {finished_at:$finished,status:"complete",ticker_count:$count}' "$batch_dir/batch.json" > "$batch_dir/complete.json"
printf 'complete batch=%s; chain=%s\n' "$batch_dir" "$batch_dir/chain.json"
```

如有失败，`complete.json` 不会生成；保留目录内的原生响应和 stderr，由 Agent 报告已采集数、失败点及未完成数。该样例 fail-fast、不重试；不能将失败前的部分数据宣称为完整链。响应字段异常时 jq 也会停止，应结合最后一个响应定位原因。没有合约时正常生成空链，不能误报 API 故障。

`complete.json` 表示候选请求已完成，不保证每条响应含有全部报价字段。null 保留为未知；缺失 Delta 在下一步单独记录。`chain.json` 是 Agent 派生表，各种原始 JSON-RPC envelope 保存在旁边。需要 gamma、vega、theta 等时直接读 `greeks`。报价与 volume 等字段的单位必须结合产品元数据说明。

## 在固定数据上筛选 Delta

设置 `DERIBIT_BATCH_DIR` 为刚才打印的目录、`DERIBIT_TARGET_DELTA` 为 signed target，例如 Put 常见 `-0.25`。默认返回 5 个候选，`DERIBIT_TOP` 可改为正整数。这个片段只读已完成批次，不请求行情；重新选择目标也复用这一批数据。

```bash
set -euo pipefail
: "${DERIBIT_BATCH_DIR:?set completed batch directory}"
: "${DERIBIT_TARGET_DELTA:?set signed target delta}"
option_top=${DERIBIT_TOP:-5}
jq -e '.status == "complete"' "$DERIBIT_BATCH_DIR/complete.json" >/dev/null
jq -n --argjson target "$DERIBIT_TARGET_DELTA" --argjson top "$option_top" --slurpfile batch "$DERIBIT_BATCH_DIR/complete.json" --slurpfile chain "$DERIBIT_BATCH_DIR/chain.json" '
  if ($target | type) != "number" or ($top | type) != "number" then error("target and top must be numeric")
  elif $top < 1 or ($top | floor) != $top then error("top must be a positive integer")
  else
    $chain[0] as $rows |
    [$rows[] | select((.greeks.delta | type) == "number") | . + {delta_distance: ((.greeks.delta - $target) | if . < 0 then -. else . end)}] as $eligible |
    {batch:$batch[0], target_delta:$target, top:$top, candidate_count:($rows | length), eligible_count:($eligible | length), excluded:[$rows[] | select((.greeks.delta | type) != "number") | {instrument_name,reason:"missing or nonnumeric delta"}], candidates:($eligible | sort_by(.delta_distance, .instrument_name) | .[:$top])}
  end'
```

排序键先是 `abs(delta - target)`，并列时按 instrument 名稳定排序；不添加用户没有要求的流动性或价差过滤。结果为“本批有有效 Delta 的候选中最接近目标”，存在 excluded 时不能保证覆盖全链。数据可能过时；报告原始采集窗口，不能把重新排序的时间说成行情时间。
