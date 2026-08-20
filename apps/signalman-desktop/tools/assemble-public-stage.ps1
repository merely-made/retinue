[CmdletBinding()]
param(
    [string]$DesktopBinary,

    [Parameter(Mandatory = $true)]
    [string]$LinkboyBinary,

    [Parameter(Mandatory = $true)]
    [string]$Espflash,

    [Parameter(Mandatory = $true)]
    [ValidateSet('windows-x86_64', 'macos-aarch64', 'macos-x86_64', 'linux-aarch64', 'linux-x86_64')]
    [string]$Platform,

    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [switch]$LinkboyOnly
)

$ErrorActionPreference = 'Stop'
$artifacts = @{
    'windows-x86_64' = @{
        executable = 'espflash.exe'
        binary_sha256 = '0cc03364c70a86325236f18ad1aaed17eedf267d89312c0cdabe4964f5cb758e'
        archive_sha256 = '854c82c947c20e7f337f120f0398042ed1760f2269cb0f9be3d2beae3b66fbfb'
        archive_url = 'https://github.com/esp-rs/espflash/releases/download/v4.5.0/espflash-x86_64-pc-windows-msvc.zip'
    }
    'macos-aarch64' = @{
        executable = 'espflash'
        binary_sha256 = 'ff92f62238a0bd6df543e0400d2b7b2ee4d97e53823d6165a80878134de860d1'
        archive_sha256 = '6614ff70e523a6bce5f4ccc6459b77275f5e7e900429004bb7eec463c95db28a'
        archive_url = 'https://github.com/esp-rs/espflash/releases/download/v4.5.0/espflash-aarch64-apple-darwin.zip'
    }
    'macos-x86_64' = @{
        executable = 'espflash'
        binary_sha256 = '2e6a1d52173f999a4ab6c6f6445038caa28ab143aa2ac9d03965249a257e8844'
        archive_sha256 = '3c5cb664742d883e4304d4fc611fc875b27a8f8d7d105d22da2f615eb36888a0'
        archive_url = 'https://github.com/esp-rs/espflash/releases/download/v4.5.0/espflash-x86_64-apple-darwin.zip'
    }
    'linux-aarch64' = @{
        executable = 'espflash'
        binary_sha256 = 'fbfa94acea38adaf498991be0b958bf7ff032defbb7c33c7a08051b858d787f7'
        archive_sha256 = '2d5972b9c18fc89bf253e60fe6df6a4f8db3aee5db0166b2c97b53bd21c01f09'
        archive_url = 'https://github.com/esp-rs/espflash/releases/download/v4.5.0/espflash-aarch64-unknown-linux-gnu.zip'
    }
    'linux-x86_64' = @{
        executable = 'espflash'
        binary_sha256 = 'a1b2a325cc6f64de4cb7a5e9b4fa2a0a4b1212555664c7ca50be29c5abb303bf'
        archive_sha256 = '542c5cc81f0cca384cbead1cacb7ccc9f35072a989b2de0fb95333d814272c22'
        archive_url = 'https://github.com/esp-rs/espflash/releases/download/v4.5.0/espflash-x86_64-unknown-linux-musl.zip'
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$linkboyPath = (Resolve-Path -LiteralPath $LinkboyBinary).Path
$espflashPath = (Resolve-Path -LiteralPath $Espflash).Path
$artifact = $artifacts[$Platform]

if ($LinkboyOnly -and $DesktopBinary) {
    throw 'LinkboyOnly stages must not include DesktopBinary.'
}

if (-not $LinkboyOnly -and -not $DesktopBinary) {
    throw 'Specify DesktopBinary, or use LinkboyOnly.'
}

$binaryPath = if ($DesktopBinary) {
    (Resolve-Path -LiteralPath $DesktopBinary).Path
}

if (Test-Path -LiteralPath $Destination) {
    throw "Destination already exists: $Destination"
}

$actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $espflashPath).Hash.ToLowerInvariant()
if ($actualHash -ne $artifact.binary_sha256) {
    throw "espflash digest mismatch for ${Platform}: expected $($artifact.binary_sha256), found $actualHash"
}

$stageRoot = New-Item -ItemType Directory -Path $Destination
$helperRoot = New-Item -ItemType Directory -Path (Join-Path $stageRoot.FullName "helpers\$Platform") -Force
$firmwareRoot = New-Item -ItemType Directory -Path (Join-Path $stageRoot.FullName 'firmware') -Force
$packagesRoot = New-Item -ItemType Directory -Path (Join-Path $firmwareRoot.FullName 'packages') -Force
$v4Root = New-Item -ItemType Directory -Path (Join-Path $firmwareRoot.FullName 'heltec-v4-phy') -Force
$t114Root = New-Item -ItemType Directory -Path (Join-Path $firmwareRoot.FullName 't114-phy') -Force
$desktopName = if ($Platform.StartsWith('windows-')) { 'signalman-desktop.exe' } else { 'signalman-desktop' }
$linkboyName = if ($Platform.StartsWith('windows-')) { 'linkboy.exe' } else { 'linkboy' }

if ($binaryPath) {
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $stageRoot.FullName $desktopName)
}
Copy-Item -LiteralPath $linkboyPath -Destination (Join-Path $stageRoot.FullName $linkboyName)
Copy-Item -LiteralPath $espflashPath -Destination (Join-Path $helperRoot.FullName $artifact.executable)
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\index.toml') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\heltec-v4-current.toml') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\t114-v51.toml') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\t114-v51-recovery.md') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\heltec-v4-current-recovery.md') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\hopspot-v4-0.3.4.toml') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\hopspot-v4-0.3.4') -Destination $packagesRoot.FullName -Recurse
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\meshtastic-t114-2.7.26.54e0d8d.toml') -Destination $packagesRoot.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\packages\meshtastic-t114-2.7.26.54e0d8d') -Destination $packagesRoot.FullName -Recurse
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\heltec-v4-phy\tulle-heltec-v4-phy') -Destination $v4Root.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'firmware\t114-phy\tulle-t114-phy-v51.uf2') -Destination $t114Root.FullName
Copy-Item -LiteralPath (Join-Path $repoRoot 'apps\linkboy\NOTICES.md') -Destination (Join-Path $stageRoot.FullName 'NOTICES.md')
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $stageRoot.FullName 'LICENSE')

@{
    schema = if ($LinkboyOnly) { 'retinue.linkboy-public-stage/v1' } else { 'retinue.signalman-public-stage/v1' }
    platform = $Platform
    helper = @{
        program = 'espflash'
        version = '4.5.0'
        binary_sha256 = $actualHash
        archive_sha256 = $artifact.archive_sha256
        archive_url = $artifact.archive_url
        license = 'MIT OR Apache-2.0'
    }
    recovery_cli = $linkboyName
    built_in_routes = @('uf2-mass-storage')
    catalog = 'firmware/packages/index.toml'
    packages = @(
        'retinue.heltec-v4',
        'retinue.t114',
        'prns.hopspot.heltec-v4',
        'meshtastic.heltec-mesh-node-t114'
    )
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stageRoot.FullName 'stage.json') -Encoding utf8NoBOM

Write-Output "assembled $($stageRoot.FullName)"
