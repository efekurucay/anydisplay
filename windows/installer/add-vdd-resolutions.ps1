# Adds common Android tablet and phone resolutions (landscape + portrait, 60 Hz)
# to Virtual Display Driver's vdd_settings.xml if they are not already present.
param(
    [string]$SettingsPath = "C:\VirtualDisplayDriver\vdd_settings.xml"
)

if (-not (Test-Path -LiteralPath $SettingsPath)) {
    exit 0
}

try {
    [xml]$doc = Get-Content -LiteralPath $SettingsPath -Raw -Encoding UTF8
} catch {
    exit 0
}

$resNode = $doc.vdd_settings.resolutions
if ($null -eq $resNode) {
    exit 0
}

# Common Android tablet & phone native resolutions (width, height) in both orientations.
$desired = @(
    @(2000, 1200), @(1200, 2000), # Samsung Galaxy Tab S6 Lite, etc.
    @(2200, 1440), @(1440, 2200), # TCL NXTPAPER 11 Plus, etc.
    @(2340, 1080), @(1080, 2340), # Common Android phones
    @(2400, 1080), @(1080, 2400), # Common Android phones
    @(2560, 1600), @(1600, 2560), # Galaxy Tab S7/S8/S9, Xiaomi Pad 5, etc.
    @(2800, 1752), @(1752, 2800), # Galaxy Tab S7+/S8+/S9+
    @(2880, 1800), @(1800, 2880), # Xiaomi Pad 6, etc.
    @(2960, 1848), @(1848, 2960)  # Galaxy Tab S8/S9 Ultra
)

$existing = @{}
foreach ($r in $resNode.resolution) {
    if ($r.width -and $r.height) {
        $existing["$($r.width)x$($r.height)"] = $true
    }
}

$changed = $false
foreach ($pair in $desired) {
    $w = $pair[0]
    $h = $pair[1]
    $key = "${w}x${h}"
    if (-not $existing.ContainsKey($key)) {
        $el = $doc.CreateElement("resolution")
        $ew = $doc.CreateElement("width")
        $ew.InnerText = "$w"
        $eh = $doc.CreateElement("height")
        $eh.InnerText = "$h"
        $er = $doc.CreateElement("refresh_rate")
        $er.InnerText = "60"
        [void]$el.AppendChild($ew)
        [void]$el.AppendChild($eh)
        [void]$el.AppendChild($er)
        [void]$resNode.AppendChild($el)
        $changed = $true
    }
}

if ($changed) {
    $settings = New-Object System.Xml.XmlWriterSettings
    $settings.Indent = $true
    $settings.IndentChars = "    "
    $settings.Encoding = New-Object System.Text.UTF8Encoding($false)
    $writer = [System.Xml.XmlWriter]::Create($SettingsPath, $settings)
    try {
        $doc.Save($writer)
    } finally {
        $writer.Dispose()
    }
}
