# 常用查询样例

以下为 Agent 按任务选用的代码片段，不是新增的 CLI 功能。先按 Skill 定位 binary，将绝对路径存入 `DERIBIT_BIN`。示例不应整页执行；只运行用户需要的调用。Shell 命令均不使用反斜杠续行。

## 单接口

```sh
"$DERIBIT_BIN" public get-index-price --index-name btc_usd
"$DERIBIT_BIN" public ticker --instrument-name BTC-PERPETUAL
"$DERIBIT_BIN" --timeout 5s public get-order-book --instrument-name BTC-PERPETUAL --depth 5
"$DERIBIT_BIN" --output table --color never public ticker --instrument-name BTC-PERPETUAL
"$DERIBIT_BIN" public get-instruments --currency BTC --kind option --expired false
"$DERIBIT_BIN" public get-expirations --currency BTC --kind option
"$DERIBIT_BIN" public get-book-summary-by-currency --currency BTC --kind option
"$DERIBIT_BIN" public get-historical-volatility --currency BTC
"$DERIBIT_BIN" public get-combo-ids --currency BTC --state active
```

合约 `BTC-PERPETUAL` 只是官方命名样例；到期期权的标识应从当前元数据取得，不能复用过期的示例名。Combo 详情需要从列表取得真实 `combo_id`。

## 原生参数与错误检查

以下三行是互相替代的调用方式；`order-book-params.json` 的内容应为 `{"instrument_name":"BTC-PERPETUAL","depth":5}`。

```sh
"$DERIBIT_BIN" --params '{"instrument_name":"BTC-PERPETUAL","depth":5}' public get-order-book
"$DERIBIT_BIN" --params-file ./order-book-params.json public get-order-book
printf '%s\n' '{"instrument_name":"BTC-PERPETUAL","depth":5}' | "$DERIBIT_BIN" --params-stdin public get-order-book
```

使用独立临时目录保留原生响应与诊断。先确认退出码，再读取 `.result`；不要直接把 CLI 管道接入 `jq` 后忽略 CLI 的退出状态。

```sh
query_dir=$(mktemp -d)
if "$DERIBIT_BIN" --output json public ticker --instrument-name BTC-PERPETUAL > "$query_dir/response.json" 2> "$query_dir/stderr.txt"; then
  jq -e 'if has("error") or (has("result") | not) then error("not a successful RPC response") else .result end' "$query_dir/response.json"
else
  query_rc=$?
  printf 'deribit-cli failed: exit=%s; evidence=%s\n' "$query_rc" "$query_dir" >&2
fi
```

该片段仅示范结果分支；若嵌入自动化，应在失败分支传播 `query_rc`，不能继续计算。`jq -e` 会对合法的 `false`/`null` 结果给出非零状态；跨方法的通用成功判断应使用 `has("result") and (has("error") | not)`，再单独读取 payload。本例 ticker 的成功结果应为 object。

## 历史区间

先把用户指定的起止时间（含时区）转换为 Unix 毫秒，赋给 `DERIBIT_START_MS` 与 `DERIBIT_END_MS`；确认起点小于终点。不要用本地日期字符串直接传参。下面是 Bash 的必填变量检查；在独立 shell 中运行，不改变交互 shell 的错误处理设置。

```bash
: "${DERIBIT_START_MS:?set start Unix milliseconds}"
: "${DERIBIT_END_MS:?set end Unix milliseconds}"
"$DERIBIT_BIN" public get-funding-rate-history --instrument-name BTC-PERPETUAL --start-timestamp "$DERIBIT_START_MS" --end-timestamp "$DERIBIT_END_MS"
"$DERIBIT_BIN" public get-volatility-index-data --currency BTC --start-timestamp "$DERIBIT_START_MS" --end-timestamp "$DERIBIT_END_MS" --resolution 3600
```

这两个命令是不同查询，按需选用。DVOL 的 `resolution` 使用秒或 `1D`；不要套用 K 线接口以分钟为单位的 resolution。观察响应的 continuation/时间范围决定是否需要下一页，避免声称一次结果已覆盖整个历史区间。

## 期权链的两种信息量

- 仅需报价汇总：保存一次 `get-instruments` 和一次 `get-book-summary-by-currency` 响应，以 `instrument_name` 关联，先按元数据的到期时间和标的筛选。保留缺少行情的合约及其 missing 状态，不把缺失 bid/ask 当作零。
- 需要 Greeks/Delta：使用 [期权组合样例](options.md) 的逐合约 ticker 采集。不要假定 book summary 含有 Greeks，也不要为了减少请求而只取部分执行价却声称全链最接近目标。
