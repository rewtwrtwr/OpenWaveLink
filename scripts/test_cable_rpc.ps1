# Live RPC check for the virtual-cable methods.
# Sends real JSON-RPC frames over the daemon's WebSocket and prints the replies.

param(
    [string]$Url = 'ws://127.0.0.1:1824'
)

$ErrorActionPreference = 'Stop'

$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = [System.Threading.CancellationToken]::None
$ws.ConnectAsync([Uri]$Url, $ct).Wait()
Write-Host "connected to $Url"

function Invoke-Rpc {
    param([int]$Id, [string]$Method, $Params)

    $payload = @{ jsonrpc = '2.0'; id = $Id; method = $Method }
    if ($null -ne $Params) { $payload.params = $Params }
    $json = $payload | ConvertTo-Json -Compress -Depth 8

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $seg = [ArraySegment[byte]]::new($bytes)
    $ws.SendAsync($seg, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).Wait()

    # Read until a frame carries our id (skip notifications).
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        $buffer = New-Object byte[] 262144
        $rseg = [ArraySegment[byte]]::new($buffer)
        $result = $ws.ReceiveAsync($rseg, $ct).Result
        $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count)
        if ($text -match "`"id`"\s*:\s*$Id\b") {
            return $text
        }
    }
    return '(timeout waiting for reply)'
}

Write-Host "`n=== 1. owl.listCables (before) ==="
Invoke-Rpc -Id 1 -Method 'owl.listCables'

Write-Host "`n=== 2. owl.createCable { names: ["OWL Spotify","OWL Personal Mix"] } ==="
Invoke-Rpc -Id 2 -Method 'owl.createCable' -Params @{ names = @('OWL Spotify', 'OWL Personal Mix') }

Write-Host "`n=== 3. owl.listCables (after) ==="
Invoke-Rpc -Id 3 -Method 'owl.listCables'

$ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done', $ct).Wait()
Write-Host "`nclosed"
