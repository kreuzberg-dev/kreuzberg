#!/usr/bin/env pwsh

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Write-Host "::group::Installing Windows dependencies"

function Retry-Command {
  param(
    [scriptblock]$Command,
    [int]$MaxAttempts = 3,
    [int]$DelaySeconds = 5
  )

  $attempt = 1
  while ($attempt -le $MaxAttempts) {
    try {
      Write-Host "Attempt $attempt of $MaxAttempts..."
      & $Command
      return $true
    }
    catch {
      $attempt++
      if ($attempt -le $MaxAttempts) {
        $backoffDelay = $DelaySeconds * [Math]::Pow(2, $attempt - 1)
        Write-Host "⚠ Attempt failed, retrying in ${backoffDelay}s..." -ForegroundColor Yellow
        Start-Sleep -Seconds $backoffDelay
      }
      else {
        return $false
      }
    }
  }
}

$tesseractCacheHit = $env:TESSERACT_CACHE_HIT -eq "true"
$llvmCacheHit = $env:LLVM_CACHE_HIT -eq "true"
$cmakeCacheHit = $env:CMAKE_CACHE_HIT -eq "true"
$cmakeInstalled = $false

Write-Host "Cache status:"
Write-Host "  TESSERACT_CACHE_HIT: $env:TESSERACT_CACHE_HIT (evaluated: $tesseractCacheHit)"
Write-Host "  LLVM_CACHE_HIT: $env:LLVM_CACHE_HIT (evaluated: $llvmCacheHit)"
Write-Host "  CMAKE_CACHE_HIT: $env:CMAKE_CACHE_HIT (evaluated: $cmakeCacheHit)"
Write-Host ""
try {
  & cmake --version 2>$null
  Write-Host "✓ CMake already installed"
  $cmakeInstalled = $true
}
catch {
  Write-Host "CMake not found, will attempt to install"
}

if (-not $tesseractCacheHit) {
  Write-Host "Tesseract cache miss, installing (optional for build - needed for tests only)..."
  if (-not (Retry-Command { choco install -y tesseract --no-progress } -MaxAttempts 3)) {
    Write-Host "::warning::Failed to install Tesseract (optional dependency - gem build does not require it)"
  }
  else {
    Write-Host "✓ Tesseract installed"
    $tesseractPath = "C:\Program Files\Tesseract-OCR"
    if (Test-Path $tesseractPath) {
      Write-Host "  Configuring Tesseract data paths..."

      $tessdataPath = "$tesseractPath\tessdata"
      if (-not (Test-Path $tessdataPath)) {
        Write-Host "  Creating tessdata directory at: $tessdataPath"
        New-Item -ItemType Directory -Path $tessdataPath -Force | Out-Null
      }

      if (-not (Test-Path "$tessdataPath\eng.traineddata")) {
        Write-Host "  Downloading English language data..."
        try {
          $engUrl = "https://github.com/tesseract-ocr/tessdata_fast/raw/main/eng.traineddata"
          Invoke-WebRequest -Uri $engUrl -OutFile "$tessdataPath\eng.traineddata" -ErrorAction Stop
          Write-Host "  ✓ Downloaded eng.traineddata"
        }
        catch {
          Write-Host "  ::warning::Failed to download eng.traineddata: $($_.Exception.Message)"
        }
      }

      if (-not (Test-Path "$tessdataPath\osd.traineddata")) {
        Write-Host "  Downloading OSD data..."
        try {
          $osdUrl = "https://github.com/tesseract-ocr/tessdata_fast/raw/main/osd.traineddata"
          Invoke-WebRequest -Uri $osdUrl -OutFile "$tessdataPath\osd.traineddata" -ErrorAction Stop
          Write-Host "  ✓ Downloaded osd.traineddata"
        }
        catch {
          Write-Host "  ::warning::Failed to download osd.traineddata: $($_.Exception.Message)"
        }
      }
    }
  }
}
else {
  Write-Host "✓ Tesseract found in cache"
}

if (-not $llvmCacheHit) {
  Write-Host "LLVM cache miss, installing LLVM/Clang (required for bindgen)..."
  if (-not (Retry-Command { choco install -y llvm --no-progress } -MaxAttempts 3)) {
    Write-Host "::warning::Failed to install LLVM/Clang via Chocolatey"
  }
  else {
    Write-Host "✓ LLVM/Clang installed"
  }
}
else {
  Write-Host "✓ LLVM/Clang found in cache"
}

$libheifCacheHit = $env:LIBHEIF_CACHE_HIT -eq "true"
if (-not $libheifCacheHit) {
  Write-Host "libheif/boost/zlib cache miss, installing via vcpkg..."
  $vcpkgRoot = if ($env:VCPKG_INSTALLATION_ROOT) { $env:VCPKG_INSTALLATION_ROOT } else { "C:\vcpkg" }
  $vcpkgExe = Join-Path $vcpkgRoot "vcpkg.exe"
  if (-not (Test-Path $vcpkgExe)) {
    Write-Host "::warning::vcpkg.exe not found at $vcpkgExe; skipping libheif/boost/zlib install"
  }
  else {
    if (-not (Retry-Command { & $vcpkgExe install "libheif:x64-windows-static-md" --recurse } -MaxAttempts 2)) {
      Write-Host "::warning::Failed to install libheif via vcpkg after retries (Windows wheels for python/node/ruby/php/elixir/swift/jni may fail to link)"
    }
    else {
      Write-Host "✓ libheif installed via vcpkg (x64-windows-static-md)"
    }
    # boost-spirit + boost-serialization (header-only, used by librevenge/libwpd:
    # RVNGBinaryData.cpp needs boost/archive/iterators/*) and zlib (linked
    # by librevenge's zip stream) — needed by xberg-libwpd's build.rs.
    if (-not (Retry-Command { & $vcpkgExe install "boost-spirit:x64-windows-static-md" "boost-serialization:x64-windows-static-md" "zlib:x64-windows-static-md" --recurse } -MaxAttempts 2)) {
      Write-Host "::warning::Failed to install boost/zlib via vcpkg after retries (xberg-libwpd build will fail)"
    }
    else {
      Write-Host "✓ boost-spirit + boost-serialization + zlib installed via vcpkg (x64-windows-static-md)"
    }
  }
  Add-Content -Path $env:GITHUB_ENV -Value "VCPKG_ROOT=$vcpkgRoot"
}
else {
  Write-Host "✓ libheif/boost/zlib found in cache"
  $vcpkgRoot = if ($env:VCPKG_INSTALLATION_ROOT) { $env:VCPKG_INSTALLATION_ROOT } else { "C:\vcpkg" }
  Add-Content -Path $env:GITHUB_ENV -Value "VCPKG_ROOT=$vcpkgRoot"
}

Write-Host "Installing PHP..."
$phpInstalled = $false
try {
  & php --version 2>$null
  Write-Host "✓ PHP already installed"
  $phpInstalled = $true
}
catch {
  Write-Host "PHP not found, installing via Chocolatey..."
  if (-not (Retry-Command { choco install -y php --no-progress } -MaxAttempts 3)) {
    Write-Host "::warning::Failed to install PHP via Chocolatey, will rely on shivammathur/setup-php action"
  }
  else {
    Write-Host "✓ PHP installed via Chocolatey"
    $phpInstalled = $true
  }
}

Write-Host "Installing CMake..."
if (-not $cmakeCacheHit) {
  Write-Host "CMake cache miss, installing..."
  if (-not (Retry-Command { choco install -y cmake --no-progress } -MaxAttempts 3)) {
    throw "Failed to install CMake after 3 attempts"
  }
  Write-Host "✓ CMake installed"
}
else {
  Write-Host "✓ CMake found in cache"
}

Write-Host "Configuring PATH and environment variables..."
$paths = @(
  "C:\Program Files\CMake\bin",
  "C:\Program Files\Tesseract-OCR",
  "C:\Program Files\LLVM\bin",
  "C:\tools\php",
  "C:\Program Files\PHP"
)

foreach ($path in $paths) {
  if (Test-Path $path) {
    Write-Host "  Adding to PATH: $path"
    Write-Output $path | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
    $env:PATH = "$path;$env:PATH"
  }
  else {
    Write-Host "  Path not found (skipping): $path"
  }
}

$tesseractPath = "C:\Program Files\Tesseract-OCR"
if (Test-Path $tesseractPath) {
  $tessdataPath = "$tesseractPath\tessdata"
  if (Test-Path $tessdataPath) {
    Write-Host "  Setting TESSDATA_PREFIX for tests: $tessdataPath"
    Add-Content -Path $env:GITHUB_ENV -Value "TESSDATA_PREFIX=$tessdataPath"
    $env:TESSDATA_PREFIX = $tessdataPath
  }
}

Write-Host "::endgroup::"

Write-Host "::group::Verifying Windows installations"

Write-Host "Tesseract (optional for build):"
try {
  $tesseractCmd = Get-Command tesseract -ErrorAction Stop
  $tesseractPath = $tesseractCmd.Path
  Write-Host "  Found at: $tesseractPath"
  Write-Host "  Command type: $($tesseractCmd.CommandType)"

  $tesseractDir = Split-Path -Parent $tesseractPath
  Write-Host "  Installation directory: $tesseractDir"

  $tessdataPath = Join-Path $tesseractDir "tessdata"
  if (Test-Path $tessdataPath) {
    Write-Host "  tessdata directory: $tessdataPath"
    Write-Host "  Available language files:"
    Get-ChildItem "$tessdataPath\*.traineddata" -ErrorAction SilentlyContinue | ForEach-Object {
      Write-Host "    - $($_.Name)"
    }
  }
  else {
    Write-Host "  tessdata directory not found at: $tessdataPath"
  }

  try {
    $version = & tesseract --version 2>&1
    Write-Host "  Version output: $version"
    Write-Host "✓ Tesseract available and working"

    Write-Host ""
    Write-Host "Available Tesseract languages:"
    & tesseract --list-langs 2>&1 | ForEach-Object { Write-Host "  $_" }
  }
  catch {
    Write-Host "⚠ Warning: Tesseract found but failed to run: $($_.Exception.Message)"
  }

  if (Test-Path $tessdataPath) {
    Write-Host ""
    Write-Host "Setting TESSDATA_PREFIX environment variable..."
    Add-Content -Path $env:GITHUB_ENV -Value "TESSDATA_PREFIX=$tessdataPath"
    Write-Host "✓ Set TESSDATA_PREFIX=$tessdataPath in GITHUB_ENV"
    $env:TESSDATA_PREFIX = $tessdataPath
  }
}
catch {
  Write-Host "⚠ Tesseract not found on PATH (not required for build)"
  Write-Host "  Error details: $($_.Exception.Message)"
  Write-Host "  Searching common installation locations..."

  $commonPaths = @(
    "C:\Program Files\Tesseract-OCR\tesseract.exe",
    "C:\Program Files (x86)\Tesseract-OCR\tesseract.exe",
    "${env:ProgramFiles}\Tesseract-OCR\tesseract.exe",
    "${env:ProgramFiles(x86)}\Tesseract-OCR\tesseract.exe"
  )

  $found = $false
  foreach ($path in $commonPaths) {
    if (Test-Path $path) {
      Write-Host "  Found Tesseract at: $path (not on PATH)"
      $tesseractDir = Split-Path -Parent $path
      $tessdataPath = Join-Path $tesseractDir "tessdata"
      if (Test-Path $tessdataPath) {
        Write-Host "  Found tessdata at: $tessdataPath"
        Add-Content -Path $env:GITHUB_ENV -Value "TESSDATA_PREFIX=$tessdataPath"
        Write-Host "✓ Set TESSDATA_PREFIX=$tessdataPath in GITHUB_ENV"
        $env:TESSDATA_PREFIX = $tessdataPath
      }
      $found = $true
      break
    }
  }

  if (-not $found) {
    Write-Host "  Tesseract not found in common locations"
  }
}

Write-Host ""
Write-Host "CMake:"
try {
  & cmake --version
  Write-Host "✓ CMake available"
  $cmakePath = (Get-Command cmake -ErrorAction Stop).Source
  if ($cmakePath) {
    Add-Content -Path $env:GITHUB_ENV -Value "CMAKE=$cmakePath"
    Write-Host "✓ Set CMAKE=$cmakePath in GITHUB_ENV"
  }
}
catch {
  Write-Host "::error::CMake not found after installation"
  throw "CMake verification failed"
}

Write-Host ""
Write-Host "Clang:"
try {
  & clang --version
  Write-Host "✓ Clang available"
}
catch {
  Write-Host "⚠ Warning: Clang not currently available on PATH"
}

Write-Host ""
Write-Host "PHP:"
try {
  & php --version
  Write-Host "✓ PHP available"
}
catch {
  Write-Host "⚠ Warning: PHP not currently available on PATH (will be set up by shivammathur/setup-php action)"
}

Write-Host "::endgroup::"
