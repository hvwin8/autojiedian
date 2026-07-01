param(
  [string]$RepoRoot = "E:\AI\xm\sxm\autojiedian"
)

$ErrorActionPreference = "Stop"
$sshConfig = Join-Path $env:USERPROFILE ".ssh\config"
$keyPath = Join-Path $RepoRoot ".secrets\ssh\id_ed25519_autojiedian_hvwin8"
if (!(Test-Path $keyPath)) { throw "Missing deploy key: $keyPath" }

New-Item -ItemType Directory -Force -Path (Split-Path $sshConfig) | Out-Null
$block = @"

Host github-autojiedian-hvwin8
  HostName ssh.github.com
  Port 443
  User git
  IdentityFile E:/AI/xm/sxm/autojiedian/.secrets/ssh/id_ed25519_autojiedian_hvwin8
  IdentitiesOnly yes
"@
$content = if (Test-Path $sshConfig) { Get-Content -Raw -LiteralPath $sshConfig } else { "" }
if ($content -notmatch "Host\s+github-autojiedian-hvwin8") {
  Add-Content -LiteralPath $sshConfig -Value $block -Encoding UTF8
} else {
  $content = $content -replace "IdentityFile .*id_ed25519_autojiedian_hvwin8", "IdentityFile E:/AI/xm/sxm/autojiedian/.secrets/ssh/id_ed25519_autojiedian_hvwin8"
  Set-Content -Encoding UTF8 -NoNewline -LiteralPath $sshConfig -Value $content
}

$user = (& whoami).Trim()
icacls $keyPath /inheritance:r | Out-Null
icacls $keyPath /grant:r "${user}:F" | Out-Null
ssh-keygen -y -f $keyPath | Out-Null
git -C $RepoRoot remote set-url origin git@github-autojiedian-hvwin8:hvwin8/autojiedian.git
Write-Host "autojiedian Git SSH config restored."
