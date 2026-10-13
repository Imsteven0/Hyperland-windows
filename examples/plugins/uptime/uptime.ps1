# Prints JSON for the widget: text, tooltip, icon, class, percentage.
$boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
$up = (Get-Date) - $boot
@{
    text    = "{0}d {1}h {2}m" -f $up.Days, $up.Hours, $up.Minutes
    tooltip = "Since " + $boot.ToString("g")
    class   = $(if ($up.TotalDays -gt 14) { "warning" } else { "" })
} | ConvertTo-Json -Compress
