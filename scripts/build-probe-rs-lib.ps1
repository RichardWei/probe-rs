param(
  [switch]$Clean,
  [switch]$Zip
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location (Join-Path $root '..')

# Clean if requested
if ($Clean) {
  Write-Host 'Cleaning probe-rs-lib...'
  cargo clean -p probe-rs-lib
  $distPath = Join-Path (Get-Location) 'dist\probe-rs-lib'
  if (Test-Path $distPath) {
    Write-Host "Removing stale dist at $distPath"
    Remove-Item -Recurse -Force $distPath
  }
}

# Build debug and release
Write-Host 'Building probe-rs-lib (debug)...'
cargo build -p probe-rs-lib
if ($LASTEXITCODE -ne 0) { Write-Error 'Debug build failed'; exit 1 }

Write-Host 'Building probe-rs-lib (release)...'
cargo build -p probe-rs-lib --release
if ($LASTEXITCODE -ne 0) { Write-Error 'Release build failed'; exit 1 }

# Prepare dist structure
$workspace = Get-Location
$targetDebug = Join-Path $workspace 'target\debug'
$targetRelease = Join-Path $workspace 'target\release'
$dist = Join-Path $workspace 'dist\probe-rs-lib'
$distInclude = Join-Path $dist 'include'
$distLibDebug = Join-Path $dist 'lib\debug'
$distLibRelease = Join-Path $dist 'lib\release'
$distBin = Join-Path $dist 'bin'
$distBinDebug = Join-Path $distBin 'debug'
$distBinRelease = Join-Path $distBin 'release'

foreach ($d in @($dist, $distInclude, $distLibDebug, $distLibRelease, $distBin, $distBinDebug, $distBinRelease)) {
  New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# Copy headers (.h/.hpp)
$headers = Get-ChildItem -Path (Join-Path $workspace 'probe-rs-lib\include') -File -Recurse -Include *.h,*.hpp
if (-not $headers) { Write-Error 'Header files not found in probe-rs-lib\include'; exit 1 }
Copy-Item $headers.FullName -Destination $distInclude -Force

# Detect platform and copy libraries
$onWindows = $env:OS -eq 'Windows_NT'
if ($onWindows) {
  # Dynamic libraries (.dll) and import libs (.dll.lib)
  $dllDebug = Join-Path $targetDebug 'probe_rs_lib.dll'
  $dllRelease = Join-Path $targetRelease 'probe_rs_lib.dll'
  if (-not (Test-Path $dllDebug)) { Write-Error "Debug DLL not found: $dllDebug"; exit 1 }
  if (-not (Test-Path $dllRelease)) { Write-Error "Release DLL not found: $dllRelease"; exit 1 }
  Copy-Item $dllDebug -Destination $distBinDebug -Force
  Copy-Item $dllRelease -Destination $distBinRelease -Force

  # Prefer import libraries named .dll.lib; fallback to .lib if present
  $impDebug = Join-Path $targetDebug 'probe_rs_lib.dll.lib'
  $impRelease = Join-Path $targetRelease 'probe_rs_lib.dll.lib'
  if (Test-Path $impDebug) {
    Copy-Item $impDebug -Destination $distLibDebug -Force
  } elseif (Test-Path (Join-Path $targetDebug 'probe_rs_lib.lib')) {
    Copy-Item (Join-Path $targetDebug 'probe_rs_lib.lib') -Destination $distLibDebug -Force
  } else {
    Write-Error 'Debug import library not found (.dll.lib or .lib)'; exit 1
  }

  if (Test-Path $impRelease) {
    Copy-Item $impRelease -Destination $distLibRelease -Force
  } elseif (Test-Path (Join-Path $targetRelease 'probe_rs_lib.lib')) {
    Copy-Item (Join-Path $targetRelease 'probe_rs_lib.lib') -Destination $distLibRelease -Force
  } else {
    Write-Error 'Release import library not found (.dll.lib or .lib)'; exit 1
  }
} else {
  # Unix-like: dynamic libraries (.so/.dylib)
  $soDebug = Get-ChildItem $targetDebug -Include *.so,*.dylib -ErrorAction SilentlyContinue | Select-Object -First 1
  $soRelease = Get-ChildItem $targetRelease -Include *.so,*.dylib -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $soDebug) { Write-Error 'Debug shared library (.so/.dylib) not found'; exit 1 }
  if (-not $soRelease) { Write-Error 'Release shared library (.so/.dylib) not found'; exit 1 }
  Copy-Item $soDebug.FullName -Destination $distBinDebug -Force
  Copy-Item $soRelease.FullName -Destination $distBinRelease -Force
}

# Verify outputs
if (-not (Get-ChildItem $distLibDebug -File)) { Write-Error 'No import libs in lib/debug'; exit 1 }
if (-not (Get-ChildItem $distLibRelease -File)) { Write-Error 'No import libs in lib/release'; exit 1 }
if (-not (Get-ChildItem $distInclude -File)) { Write-Error 'No header files in include'; exit 1 }
if (-not (Get-ChildItem $distBinDebug -File)) { Write-Error 'No binaries in bin/debug'; exit 1 }
if (-not (Get-ChildItem $distBinRelease -File)) { Write-Error 'No binaries in bin/release'; exit 1 }

# Optionally zip
if ($Zip) {
  $zipPath = Join-Path $workspace 'dist\probe-rs-lib.zip'
  if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
Compress-Archive -Path (Join-Path $dist '*') -DestinationPath $zipPath
  if (-not (Test-Path $zipPath)) { Write-Error 'Failed to create zip archive'; exit 1 }
}

Write-Host "Artifacts organized under $dist"