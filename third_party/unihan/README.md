# Unicode Unihan 读音摘录

匹配引擎的拼音表在构建 `lanwork-core` 时由本目录的摘录生成。程序不在运行时下载或解析 Unihan。

## 来源

- Unicode 版本：18.0.0
- 文件内日期：2026-07-31
- 下载：<https://www.unicode.org/Public/18.0.0/ucd/Unihan.zip>
- 压缩包内文件：`Unihan_Readings.txt`
- 上游 `Unihan_Readings.txt` 的 SHA-256：`9d39995b5de714e8ce93716ed5d15eaa0792d68e407cdf8a0add2893b8f4150b`
- 下载当时压缩包的 SHA-256：`4c93ea9c1f636451729a840978f1667a53886af37ba854fdcce109721c63d43e`（构建不读取压缩包）

仓库不存放这份全文。构建只用 `kMandarin` 和 `kHanyuPinyin`，并且只要 CJK 扩展 A（U+3400–U+4DBF）和基本区（U+4E00–U+9FFF）。摘录是 [kMandarin_kHanyuPinyin.txt](kMandarin_kHanyuPinyin.txt)，数据行从上游原样复制。摘录 SHA-256：`a5ad0750009db5a4461efc9c66a87c359b9cf7e6e972a6172ba45c673556fa18`。构建校验的是摘录，不是上游全文。

重现摘录：

```text
curl -fsSL -o Unihan.zip https://www.unicode.org/Public/18.0.0/ucd/Unihan.zip
unzip -p Unihan.zip Unihan_Readings.txt > Unihan_Readings.txt
python3 extract.py Unihan_Readings.txt kMandarin_kHanyuPinyin.txt
sha256sum kMandarin_kHanyuPinyin.txt
```

`extract.py` 会先核对上游全文的 SHA-256。校验和变化时，先更新本说明和 `crates/core/build.rs` 里的期望值，再让构建通过。不要为了让样例通过而删掉读音。

同一码位先保留 `kMandarin` 的书写顺序，再追加 `kHanyuPinyin` 里去声调后尚未出现的读音。声调符号去掉；`ü` 写成 `v`；`ê` 写成 `e`。不按词义删读音，也不做双拼。这些规则在 `crates/core/build.rs`，不在摘录文件里改写读音。

## 许可

[LICENSE.txt](LICENSE.txt) 是 Unicode License v3 的原文，与摘录放在一起。摘录开头保留了上游文件的版权说明。本仓库只在说明数据来源时使用 Unicode 这一名称，不用它做产品推广。
