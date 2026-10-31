$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'check_vault_leaks.py'
if (Get-Command python3 -ErrorAction SilentlyContinue) {
    & python3 $checker
} elseif (Get-Command python -ErrorAction SilentlyContinue) {
    & python $checker
} else {
    throw 'Python 3 is required to run the hardware vault gate.'
}
if ($LASTEXITCODE -ne 0) {
    throw 'Hardware vault boundary check failed.'
}
