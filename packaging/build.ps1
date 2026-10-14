# 在 windows-latest 上编译 release、安装包和便携版，并静默安装再卸载。
# 不创建 GitHub Release，也不生成 minisign 签名。
# 安装包生成之后写入 latest.json：版本、发行说明、文件名、SHA-256。

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$cargo = Get-Content -Raw (Join-Path $root "Cargo.toml")
$versionMatch = [regex]::Match($cargo, '(?m)^version = "([^"]+)"')
if (-not $versionMatch.Success) {
    throw "Cargo.toml 里没有 workspace 版本"
}
$version = $versionMatch.Groups[1].Value

$issPath = Join-Path $root "packaging\lanwork.iss"
$iss = Get-Content -Raw $issPath
$issVersion = [regex]::Match($iss, '#define MyAppVersion "([^"]+)"')
if (-not $issVersion.Success -or $issVersion.Groups[1].Value -ne $version) {
    throw "lanwork.iss 的 MyAppVersion 与 Cargo 版本 $version 不一致"
}

if ($env:GITHUB_REF_TYPE -eq "tag") {
    $tag = $env:GITHUB_REF_NAME
    if ($tag -ne "v$version") {
        throw "tag $tag 与 Cargo 版本 v$version 不一致"
    }
}

Write-Host "building Lanwork $version"
cargo build -p lanwork -p lanwork-shortcut --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$releaseDir = Join-Path $root "target\release"
$exe = Join-Path $releaseDir "lanwork.exe"
$helper = Join-Path $releaseDir "lanwork-shortcut.exe"
if (-not (Test-Path $exe)) { throw "缺少 $exe" }
if (-not (Test-Path $helper)) { throw "缺少 $helper" }

function Get-PeSubsystem([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 0x40 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "$Path 不是 MZ"
    }
    $e_lfanew = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($e_lfanew -lt 0 -or ($e_lfanew + 24 + 70) -gt $bytes.Length) {
        throw "$Path PE 头超出文件"
    }
    $sig = [BitConverter]::ToUInt32($bytes, $e_lfanew)
    if ($sig -ne 0x00004550) { throw "$Path 不是 PE" }
    $magic = [BitConverter]::ToUInt16($bytes, $e_lfanew + 24)
    # 0x10B 是 PE32，0x20B 是 PE32+。两种可选头里 Subsystem 都在偏移 68。
    if ($magic -ne 0x10B -and $magic -ne 0x20B) {
        throw "$Path 可选头 magic $magic"
    }
    return [BitConverter]::ToUInt16($bytes, $e_lfanew + 24 + 68)
}

function Assert-GuiSubsystem([string]$Path, [string]$Label) {
    $subsystem = Get-PeSubsystem $Path
    Write-Host "$Label PE subsystem=$subsystem"
    # IMAGE_SUBSYSTEM_WINDOWS_GUI = 2。控制台是 3。
    if ($subsystem -ne 2) {
        throw "$Label PE 子系统是 $subsystem，发布版必须是 GUI (2)"
    }
}

function Assert-ProductVersion([string]$Path, [string]$Expected, [string]$Label) {
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    $product = ($info.ProductVersion).Trim().Trim([char]0)
    $file = ($info.FileVersion).Trim().Trim([char]0)
    if ($product -ne $Expected) {
        throw "$Label ProductVersion '$product' FileVersion '$file'，期望 $Expected"
    }
    if ($file -ne $Expected -and $file -ne "$Expected.0") {
        throw "$Label FileVersion '$file'，期望 $Expected 或 $Expected.0"
    }
    # Inno 会把版本字符串用空格补到固定宽度。
    $description = ($info.FileDescription).Trim().Trim([char]0)
    $name = ($info.ProductName).Trim().Trim([char]0)
    if ($description -ne "Lanwork") {
        throw "$Label FileDescription '$description'"
    }
    if ($name -ne "Lanwork") {
        throw "$Label ProductName '$name'"
    }
}

Assert-ProductVersion $exe $version "lanwork.exe"
Assert-GuiSubsystem $exe "lanwork.exe"

$staging = Join-Path $root "packaging\staging"
$helperDir = Join-Path $root "packaging\helper"
$out = Join-Path $root "packaging\out"
New-Item -ItemType Directory -Force $helperDir | Out-Null
New-Item -ItemType Directory -Force $out | Out-Null
& $helper stage $exe $staging
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Copy-Item $helper (Join-Path $helperDir "lanwork-shortcut.exe") -Force

# windows-latest 可能预装别的 Inno Setup。不用那份 ISCC。
# 只使用哈希核对过的 6.7.3，装到独立目录。/PORTABLE=1 避免写到已有安装目录。
$innoVersion = "6.7.3"
$innoRoot = Join-Path $root "packaging\.tools\innosetup-$innoVersion"
$iscc = Join-Path $innoRoot "ISCC.exe"
$innoUrl = "https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-$innoVersion.exe"
$innoHash = "9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732"

function Get-IsccBanner([string]$Path) {
    if (-not (Test-Path $Path)) { return "" }
    # 帮助文本只有 “Inno Setup 6”，没有修订号。读完脚本、开始编译之前才打印
    # Compiler engine version。空脚本随后会编译失败，这个退出码不是打包失败。
    $PSNativeCommandUseErrorActionPreference = $false
    $probe = Join-Path $env:TEMP "lanwork-iscc-probe.iss"
    Set-Content -Path $probe -Value "; probe" -Encoding ascii
    $outFile = Join-Path $env:TEMP ("iscc-banner-" + [guid]::NewGuid().ToString() + ".txt")
    & $Path "/O-" $probe > $outFile 2>&1
    $text = ""
    if (Test-Path $outFile) {
        $text = Get-Content -Raw $outFile
        Remove-Item $outFile -Force
    }
    if (-not $text) { return "" }
    return $text
}

function Test-Iscc673([string]$Path) {
    return (Get-IsccBanner $Path) -match 'Compiler engine version: Inno Setup 6\.7\.3'
}

function Write-IsccCandidates {
    $dirs = @(
        $innoRoot,
        (Join-Path $env:USERPROFILE "Desktop\Inno Setup 6"),
        "${env:ProgramFiles(x86)}\Inno Setup 6",
        "$env:ProgramFiles\Inno Setup 6",
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6")
    )
    foreach ($dir in $dirs) {
        $candidate = Join-Path $dir "ISCC.exe"
        if (-not (Test-Path $candidate)) { continue }
        Write-Host "found ISCC Path=$candidate"
        Write-Host (Get-IsccBanner $candidate)
    }
}

$systemIscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
Write-Host "preinstalled ISCC Path=$systemIscc"
Write-Host (Get-IsccBanner $systemIscc)

if (-not (Test-Iscc673 $iscc)) {
    if (Test-Path $innoRoot) { Remove-Item -Recurse -Force $innoRoot }
    New-Item -ItemType Directory -Force $innoRoot | Out-Null
    $installer = Join-Path $env:TEMP "innosetup-$innoVersion.exe"
    Invoke-WebRequest -Uri $innoUrl -OutFile $installer
    $got = (Get-FileHash $installer -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $innoHash) {
        throw "Inno Setup 哈希不符: $got"
    }
    $innoLog = Join-Path $out "innosetup-install.log"
    # 参数串原样交给 Inno。/DIR 必须带引号，否则会落到预装目录。
    $argLine = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /PORTABLE=1 /DIR=`"$innoRoot`" /LOG=`"$innoLog`""
    Write-Host "inno installer: $argLine"
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $installer
    $psi.Arguments = $argLine
    $psi.UseShellExecute = $false
    $proc = [Diagnostics.Process]::Start($psi)
    if ($null -eq $proc) { throw "无法启动 Inno Setup 安装程序" }
    $proc.WaitForExit()
    Write-Host "inno installer exit $($proc.ExitCode)"
    if ($proc.ExitCode -ne 0) {
        $tail = ""
        if (Test-Path $innoLog) { $tail = (Get-Content -Path $innoLog -Tail 40) -join "`n" }
        throw "安装 Inno Setup $innoVersion 失败，退出码 $($proc.ExitCode)`n$tail"
    }
}
if (-not (Test-Iscc673 $iscc)) {
    foreach ($dir in @(
        (Join-Path $env:USERPROFILE "Desktop\Inno Setup 6"),
        "${env:ProgramFiles(x86)}\Inno Setup 6",
        "$env:ProgramFiles\Inno Setup 6",
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6")
    )) {
        $candidate = Join-Path $dir "ISCC.exe"
        if (-not (Test-Iscc673 $candidate)) { continue }
        if ($candidate -eq $systemIscc) { continue }
        Write-Host "copy verified Inno Setup 6.7.3 from $dir"
        Copy-Item -Path (Join-Path $dir "*") -Destination $innoRoot -Recurse -Force
        break
    }
}
if (-not (Test-Iscc673 $iscc)) {
    Write-IsccCandidates
    $tail = ""
    $innoLog = Join-Path $out "innosetup-install.log"
    if (Test-Path $innoLog) { $tail = (Get-Content -Path $innoLog -Tail 40) -join "`n" }
    throw "找不到校验过的 Inno Setup $innoVersion : $iscc`n$tail"
}
$banner = Get-IsccBanner $iscc
Write-Host "verified Inno Setup ISCC=$iscc"
Write-Host $banner
if ($banner -notmatch 'Compiler engine version: Inno Setup 6\.7\.3') {
    throw "ISCC 横幅里没有 Compiler engine version: Inno Setup 6.7.3"
}

& $iscc $issPath
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$setup = Join-Path $out "Lanwork-Setup-$version.exe"
if (-not (Test-Path $setup)) { throw "缺少 $setup" }
Assert-ProductVersion $setup $version "安装包"

$hash = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash.ToLowerInvariant()
if ($hash -notmatch '^[0-9a-f]{64}$') { throw "安装包 SHA-256 不是 64 位小写十六进制" }
$setupName = "Lanwork-Setup-$version.exe"
$notesPath = Join-Path $root "packaging\release-notes.md"
$notes = Get-Content -Raw $notesPath
if ($notes -notlike "*__VERSION__*") { throw "Release 说明缺少版本占位" }
$notes = $notes.Replace("__VERSION__", $version).Replace("__TAG__", "v$version")
if ($notes -like "*__VERSION__*" -or $notes -like "*__TAG__*") { throw "Release 说明仍有占位" }
if ($notes -notlike "*https://slint.dev*") { throw "Release 说明缺少 Slint 链接" }
if ($notes -notlike "*MadeWithSlint*") { throw "Release 说明缺少 Slint 徽标" }
if ($notes -like "*仅供自测*") { throw "Release 说明不能标成仅供自测" }
$latestPath = Join-Path $out "latest.json"
$payload = @{
    version   = $version
    notes     = $notes
    installer = @{
        name   = $setupName
        sha256 = $hash
    }
}
$latestJson = ($payload | ConvertTo-Json -Compress -Depth 5) + "`n"
$utf8 = New-Object System.Text.UTF8Encoding $false
[System.IO.File]::WriteAllText($latestPath, $latestJson, $utf8)
$latest = Get-Content -Raw -Encoding utf8 $latestPath | ConvertFrom-Json
if ($latest.version -ne $version) { throw "latest.json 的 version 不是 $version" }
if ([string]::IsNullOrWhiteSpace([string]$latest.notes)) { throw "latest.json 缺少发行说明" }
if ($latest.notes -notlike "*https://slint.dev*") { throw "latest.json 的发行说明缺少 Slint 链接" }
if ($latest.installer.name -ne $setupName) { throw "latest.json 的安装包文件名不是 $setupName" }
if ($latest.installer.sha256 -ne $hash) { throw "latest.json 的 sha256 对不上" }
$again = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash.ToLowerInvariant()
if ($again -ne $hash) { throw "安装包在写入 latest.json 之后变了" }

$zip = Join-Path $out "Lanwork-$version-portable-x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $zip
if ($zip -notlike "*portable*") { throw "便携版文件名不含 portable" }

[System.IO.File]::WriteAllText((Join-Path $out "RELEASE.md"), $notes)
Set-Content -Path (Join-Path $out "version.txt") -Value $version -Encoding ascii

$installDir = Join-Path $env:TEMP "Lanwork 测试"
$log = Join-Path $out "install.log"
if (Test-Path $installDir) { Remove-Item -Recurse -Force $installDir }

$docs = Join-Path $env:USERPROFILE "Documents\Lanwork"
$cache = Join-Path $env:LOCALAPPDATA "Lanwork"
New-Item -ItemType Directory -Force $docs | Out-Null
New-Item -ItemType Directory -Force $cache | Out-Null
$docsMarker = Join-Path $docs "packaging-keep.txt"
$cacheMarker = Join-Path $cache "packaging-keep.txt"
Set-Content -Path $docsMarker -Value "keep" -Encoding utf8
Set-Content -Path $cacheMarker -Value "keep" -Encoding utf8

$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
if (-not (Test-Path $runKey)) { New-Item -Path $runKey -Force | Out-Null }
New-ItemProperty -Path $runKey -Name "Lanwork" -PropertyType String -Value "sentinel" -Force | Out-Null
New-ItemProperty -Path $runKey -Name "LanworkPackagingProbe" -PropertyType String -Value "keep" -Force | Out-Null

# 安装包和卸载程序是图形程序。& 不会等待它们退出。需要 PowerShell 7 的 ArgumentList。
function Invoke-Gui([string]$File, [string[]]$Arguments, [string]$Log) {
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $File
    $info.UseShellExecute = $false
    foreach ($arg in $Arguments) {
        [void]$info.ArgumentList.Add($arg)
    }
    $proc = [Diagnostics.Process]::Start($info)
    if ($null -eq $proc) { throw "无法启动 $File" }
    $proc.WaitForExit()
    if ($proc.ExitCode -ne 0) {
        $tail = ""
        if ($Log -and (Test-Path $Log)) {
            $tail = (Get-Content -Path $Log -Tail 40) -join "`n"
        }
        throw "$File 退出码 $($proc.ExitCode)`n$tail"
    }
}

Write-Host "silent install to $installDir"
Invoke-Gui $setup @(
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/DIR=$installDir",
    "/LOG=$log"
) $log

foreach ($name in @(
    "lanwork.exe",
    "Everything64.dll",
    "Everything3_x64.dll",
    "Everything-SDK-LICENSE.txt",
    "Everything-SDK3-LICENSE.txt",
    "Unicode-LICENSE.txt",
    "THIRD_PARTY_NOTICES.txt"
)) {
    if (-not (Test-Path (Join-Path $installDir $name))) {
        throw "安装目录缺少 $name"
    }
}
if (Test-Path (Join-Path $installDir "lanwork-shortcut.exe")) {
    throw "快捷方式工具不应留在安装目录"
}
Assert-ProductVersion (Join-Path $installDir "lanwork.exe") $version "已安装的 lanwork.exe"
Assert-GuiSubsystem (Join-Path $installDir "lanwork.exe") "已安装的 lanwork.exe"

$aumidKey = "HKCU:\Software\Classes\AppUserModelId\Lanwork"
$aumid = Get-ItemProperty $aumidKey
if ($aumid.DisplayName -ne "Lanwork") { throw "DisplayName 不符" }
if ($aumid.CustomActivator -ne "{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}") {
    throw "CustomActivator 不符: $($aumid.CustomActivator)"
}
$clsidKey = "HKCU:\Software\Classes\CLSID\{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}\LocalServer32"
$localServer = (Get-Item $clsidKey).GetValue("")
$expectedServer = '"' + (Join-Path $installDir "lanwork.exe") + '"'
if ($localServer -ne $expectedServer) {
    throw "LocalServer32 '$localServer' != '$expectedServer'"
}
$runAfter = (Get-ItemProperty $runKey -Name "Lanwork").Lanwork
if ($runAfter -ne "sentinel") { throw "安装改写了 Run 值: $runAfter" }

$lnk = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Lanwork.lnk"
if (-not (Test-Path $lnk)) { throw "没有开始菜单快捷方式" }
& $helper read $lnk
if ($LASTEXITCODE -ne 0) { throw "快捷方式属性读回失败" }
if (-not (Test-Path $docsMarker)) { throw "安装删了数据目录标记" }
if (-not (Test-Path $cacheMarker)) { throw "安装删了缓存目录标记" }

$unlog = Join-Path $out "uninstall.log"
$unins = Join-Path $installDir "unins000.exe"
Invoke-Gui $unins @(
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/LOG=$unlog"
) $unlog

if (Test-Path (Join-Path $installDir "lanwork.exe")) { throw "卸载后程序还在" }
if (Test-Path $lnk) { throw "卸载后快捷方式还在" }
if (Test-Path $aumidKey) { throw "卸载后 AUMID 键还在" }
if (Test-Path "HKCU:\Software\Classes\CLSID\{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}") {
    throw "卸载后 CLSID 键还在"
}
$runLeft = Get-ItemProperty $runKey -Name "Lanwork" -ErrorAction SilentlyContinue
if ($null -ne $runLeft.Lanwork) { throw "卸载后 Run 值还在" }
$probe = (Get-ItemProperty $runKey -Name "LanworkPackagingProbe").LanworkPackagingProbe
if ($probe -ne "keep") { throw "卸载删了别的 Run 值" }
Remove-ItemProperty -Path $runKey -Name "LanworkPackagingProbe"
if (-not (Test-Path $docsMarker)) { throw "卸载删了数据目录" }
if (-not (Test-Path $cacheMarker)) { throw "卸载删了缓存目录" }
Remove-Item $docsMarker
Remove-Item $cacheMarker

Write-Host "package ok $setup"
Write-Host "portable ok $zip"
