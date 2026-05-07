# Local relay protocol smoke test.
#
param(
    [int]$Port = 34790,
    [string]$RelayRepo = "..\freeplay-relay"
)

# Starts the sibling freeplay-relay binary on 127.0.0.1, simulates two
# clients with separate UDP sockets, verifies REGISTERED/PEER_READY, then
# sends DATA both directions through the relay.

$ErrorActionPreference = "Stop"

function ConvertFrom-Hex {
    param([Parameter(Mandatory=$true)][string]$Hex)
    $clean = $Hex.Trim()
    if (($clean.Length % 2) -ne 0) {
        throw "hex string has odd length"
    }
    $bytes = New-Object byte[] ($clean.Length / 2)
    for ($i = 0; $i -lt $bytes.Length; $i++) {
        $bytes[$i] = [Convert]::ToByte($clean.Substring($i * 2, 2), 16)
    }
    return $bytes
}

function ConvertTo-Hex {
    param([Parameter(Mandatory=$true)][byte[]]$Bytes)
    return (($Bytes | ForEach-Object { $_.ToString("x2") }) -join "")
}

function New-RegisterPacket {
    param(
        [Parameter(Mandatory=$true)][byte]$Role,
        [Parameter(Mandatory=$true)][Int64]$Expiry,
        [Parameter(Mandatory=$true)][string]$RoomId,
        [Parameter(Mandatory=$true)][byte[]]$Secret
    )

    if ($RoomId.Length -ne 36) {
        throw "room id must be 36 bytes"
    }

    $payload = "$Role`:$Expiry`:$RoomId"
    $hmac = [System.Security.Cryptography.HMACSHA256]::new($Secret)
    $mac = $hmac.ComputeHash([System.Text.Encoding]::ASCII.GetBytes($payload))

    $pkt = New-Object byte[] 78
    $pkt[0] = 0x01
    $pkt[1] = $Role
    $expiryBytes = [BitConverter]::GetBytes($Expiry)
    if ([BitConverter]::IsLittleEndian) {
        [Array]::Reverse($expiryBytes)
    }
    [Array]::Copy($expiryBytes, 0, $pkt, 2, 8)
    [Array]::Copy([System.Text.Encoding]::ASCII.GetBytes($RoomId), 0, $pkt, 10, 36)
    [Array]::Copy($mac, 0, $pkt, 46, 32)
    return $pkt
}

function Receive-Tag {
    param(
        [Parameter(Mandatory=$true)][System.Net.Sockets.UdpClient]$Client,
        [Parameter(Mandatory=$true)][byte]$ExpectedTag,
        [Parameter(Mandatory=$true)][string]$Label
    )

    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $from = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 0)
            $pkt = $Client.Receive([ref]$from)
            if ($pkt.Length -gt 0 -and $pkt[0] -eq $ExpectedTag) {
                return $pkt
            }
            $tag = if ($pkt.Length -gt 0) { "0x{0:x2}" -f $pkt[0] } else { "empty" }
            Write-Host "  ignored $Label packet tag $tag from $from"
        } catch [System.Net.Sockets.SocketException] {
            if ($_.Exception.SocketErrorCode -ne [System.Net.Sockets.SocketError]::TimedOut) {
                throw
            }
        }
    }
    throw "timed out waiting for $Label tag 0x$($ExpectedTag.ToString('x2'))"
}

function Send-Packet {
    param(
        [Parameter(Mandatory=$true)][System.Net.Sockets.UdpClient]$Client,
        [Parameter(Mandatory=$true)][byte[]]$Packet,
        [Parameter(Mandatory=$true)][System.Net.IPEndPoint]$Relay
    )
    [void]$Client.Send($Packet, $Packet.Length, $Relay)
}

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$relayRoot = (Resolve-Path (Join-Path $root $RelayRepo)).Path
$relayExe = Join-Path $relayRoot "target\release\freeplay-relay.exe"
if (-not (Test-Path $relayExe)) {
    Write-Host "Building freeplay-relay..."
    cargo build --release --manifest-path (Join-Path $relayRoot "Cargo.toml")
}
if (-not (Test-Path $relayExe)) {
    throw "relay binary not found at $relayExe"
}

$secretHex = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
$secret = ConvertFrom-Hex $secretHex
$relayEndpoint = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Parse("127.0.0.1"), $Port)
$roomId = [Guid]::NewGuid().ToString()
$expiry = [DateTimeOffset]::UtcNow.AddMinutes(10).ToUnixTimeSeconds()

$stdout = Join-Path $env:TEMP "freeplay-relay-smoke.out.log"
$stderr = Join-Path $env:TEMP "freeplay-relay-smoke.err.log"
Remove-Item $stdout, $stderr -ErrorAction SilentlyContinue

$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $relayExe
$psi.WorkingDirectory = $relayRoot
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.Environment["RELAY_BIND"] = "127.0.0.1:$Port"
$psi.Environment["RELAY_SHARED_SECRET"] = $secretHex
$psi.Environment["RUST_LOG"] = "freeplay_relay=info"

$relayProcess = [System.Diagnostics.Process]::Start($psi)
$client0 = $null
$client1 = $null
try {
    Start-Sleep -Milliseconds 500
    if ($relayProcess.HasExited) {
        throw "relay exited early with code $($relayProcess.ExitCode)"
    }

    $client0 = [System.Net.Sockets.UdpClient]::new(0)
    $client1 = [System.Net.Sockets.UdpClient]::new(0)
    $client0.Client.ReceiveTimeout = 500
    $client1.Client.ReceiveTimeout = 500

    $reg0 = New-RegisterPacket -Role 0 -Expiry $expiry -RoomId $roomId -Secret $secret
    $reg1 = New-RegisterPacket -Role 1 -Expiry $expiry -RoomId $roomId -Secret $secret

    Write-Host "Relay smoke room $roomId on 127.0.0.1:$Port"

    Send-Packet $client0 $reg0 $relayEndpoint
    [void](Receive-Tag $client0 0x02 "role0 REGISTERED")

    Send-Packet $client1 $reg1 $relayEndpoint
    [void](Receive-Tag $client1 0x02 "role1 REGISTERED")
    [void](Receive-Tag $client1 0x05 "role1 PEER_READY")
    [void](Receive-Tag $client0 0x05 "role0 PEER_READY")

    $data0Payload = [System.Text.Encoding]::ASCII.GetBytes("hello-from-role0")
    $data0 = New-Object byte[] (1 + $data0Payload.Length)
    $data0[0] = 0x04
    [Array]::Copy($data0Payload, 0, $data0, 1, $data0Payload.Length)
    Send-Packet $client0 $data0 $relayEndpoint
    $got0 = Receive-Tag $client1 0x04 "role1 DATA"
    $body0 = [System.Text.Encoding]::ASCII.GetString($got0, 1, $got0.Length - 1)
    if ($body0 -ne "hello-from-role0") {
        throw "role1 got unexpected DATA body: $body0"
    }

    $data1Payload = [System.Text.Encoding]::ASCII.GetBytes("hello-from-role1")
    $data1 = New-Object byte[] (1 + $data1Payload.Length)
    $data1[0] = 0x04
    [Array]::Copy($data1Payload, 0, $data1, 1, $data1Payload.Length)
    Send-Packet $client1 $data1 $relayEndpoint
    $got1 = Receive-Tag $client0 0x04 "role0 DATA"
    $body1 = [System.Text.Encoding]::ASCII.GetString($got1, 1, $got1.Length - 1)
    if ($body1 -ne "hello-from-role1") {
        throw "role0 got unexpected DATA body: $body1"
    }

    Write-Host "relay smoke: OK"
} finally {
    if ($client0) { $client0.Dispose() }
    if ($client1) { $client1.Dispose() }
    if ($relayProcess -and -not $relayProcess.HasExited) {
        $relayProcess.Kill()
        $relayProcess.WaitForExit()
    }
    if ($relayProcess) {
        $outText = $relayProcess.StandardOutput.ReadToEnd()
        $errText = $relayProcess.StandardError.ReadToEnd()
        if ($outText) { Set-Content $stdout $outText }
        if ($errText) { Set-Content $stderr $errText }
    }
}
