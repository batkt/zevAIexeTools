param(
    [string]$Title = "Диск C:\ дүүрч байна",
    [string]$Message = "95% дүүрсэн, 18.7 GB үлдсэн. Лог болон өгөгдөл бичигдэхгүй болох эрсдэлтэй.",
    [string]$Advice = "Хэрэгцээгүй файлуудыг устгаж диск чөлөөлнө үү.",
    [string]$Severity = "Critical",
    [string]$LogoPath = "C:\ProgramData\Zev Detector\parkease.png",
    [int]$TimeoutSecs = 20
)

Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase

$isCrit = ($Severity -eq "Critical")
$borderColor = if ($isCrit) { "#EF4444" } else { "#F5A623" }
$sevBadgeBg = if ($isCrit) { "#EF4444" } else { "#F59E0B" }
$sevText = if ($isCrit) { "Анхаарах шаардлагатай" } else { "АНХААРУУЛГА" }

$hasLogo = (Test-Path $LogoPath)
$logoElem = if ($hasLogo) {
    "<Border CornerRadius='10' Width='36' Height='36' Margin='0,0,12,0' Background='#F5A623' ClipToBounds='True'>
        <Image Source='$LogoPath' Stretch='UniformToFill'/>
     </Border>"
} else {
    "<Border CornerRadius='10' Width='36' Height='36' Margin='0,0,12,0' Background='#F5A623'>
        <TextBlock Text='P' Foreground='White' FontWeight='Black' FontSize='22' HorizontalAlignment='Center' VerticalAlignment='Center' FontFamily='Segoe UI Black'/>
     </Border>"
}

$safeTitle = [System.Security.SecurityElement]::Escape($Title)
$safeMsg = [System.Security.SecurityElement]::Escape($Message)
$safeAdv = [System.Security.SecurityElement]::Escape($Advice)

$msgElem = if (![string]::IsNullOrWhiteSpace($Message)) {
    "<TextBlock Text='$safeMsg' Foreground='#D4D4D8' FontSize='12' TextWrapping='Wrap' LineHeight='18' Margin='0,6,0,0' FontFamily='Segoe UI Variable Text, Segoe UI'/>"
} else {
    ""
}

$xaml = @"
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Title="ParkEase Alert" Width="480" SizeToContent="Height"
        WindowStyle="None" AllowsTransparency="True" Background="Transparent"
        Topmost="True" ShowInTaskbar="False" WindowStartupLocation="CenterScreen">
    <Border Background="#131316" CornerRadius="20" BorderBrush="$borderColor" BorderThickness="1.5" Margin="14">
        <Border.Effect>
            <DropShadowEffect BlurRadius="30" ShadowDepth="6" Opacity="0.55" Color="#000000"/>
        </Border.Effect>
        <Grid Margin="22">
            <Grid.RowDefinitions>
                <RowDefinition Height="Auto"/>
                <RowDefinition Height="Auto"/>
                <RowDefinition Height="Auto"/>
                <RowDefinition Height="Auto"/>
            </Grid.RowDefinitions>

            <!-- Header -->
            <Grid Grid.Row="0">
                <Grid.ColumnDefinitions>
                    <ColumnDefinition Width="*"/>
                    <ColumnDefinition Width="Auto"/>
                </Grid.ColumnDefinitions>
                <StackPanel Orientation="Horizontal" VerticalAlignment="Center">
                    $logoElem
                    <StackPanel VerticalAlignment="Center">
                        <StackPanel Orientation="Horizontal">
                            <TextBlock Text="ParkEase" Foreground="#F5A623" FontWeight="Black" FontSize="15" FontFamily="Segoe UI Variable Display, Segoe UI"/>
                            <TextBlock Text=" • Хяналтын систем" Foreground="#A1A1AA" FontSize="12" Margin="4,2,0,0" FontFamily="Segoe UI Variable Text, Segoe UI"/>
                        </StackPanel>
                        <Border Background="$sevBadgeBg" CornerRadius="5" HorizontalAlignment="Left" Padding="6,1" Margin="0,3,0,0">
                            <TextBlock Text="$sevText" Foreground="White" FontWeight="Bold" FontSize="9" FontFamily="Segoe UI"/>
                        </Border>
                    </StackPanel>
                </StackPanel>
                <Button Grid.Column="1" Name="BtnCloseX" Content="✕" Width="28" Height="28" Background="#27272A" Foreground="#A1A1AA" FontWeight="Bold" BorderThickness="0" Cursor="Hand">
                    <Button.Resources>
                        <Style TargetType="Border">
                            <Setter Property="CornerRadius" Value="14"/>
                        </Style>
                    </Button.Resources>
                </Button>
            </Grid>

            <!-- Title & Message -->
            <StackPanel Grid.Row="1" Margin="0,16,0,12">
                <TextBlock Text="$safeTitle" Foreground="White" FontWeight="Bold" FontSize="15" TextWrapping="Wrap" FontFamily="Segoe UI Variable Display, Segoe UI"/>
                $msgElem
            </StackPanel>

            <!-- Advice Box -->
            <Border Grid.Row="2" Background="#202024" CornerRadius="12" Padding="12,10" Margin="0,0,0,16" BorderBrush="#2E2E33" BorderThickness="1">
                <Grid>
                    <Grid.ColumnDefinitions>
                        <ColumnDefinition Width="Auto"/>
                        <ColumnDefinition Width="*"/>
                    </Grid.ColumnDefinitions>
                    <TextBlock Grid.Column="0" Text="💡" Margin="0,0,10,0" FontSize="14" VerticalAlignment="Top"/>
                    <TextBlock Grid.Column="1" Text="$safeAdv" Foreground="#F4F4F5" FontSize="11.5" TextWrapping="Wrap" LineHeight="16" FontFamily="Segoe UI"/>
                </Grid>
            </Border>

            <!-- Footer Action -->
            <Grid Grid.Row="3">
                <TextBlock Name="TxtTimer" Text="$TimeoutSecs с дараа хаагдана" Foreground="#71717A" FontSize="11" VerticalAlignment="Center" FontFamily="Segoe UI"/>
                <Button Name="BtnAck" Content="Хаах" HorizontalAlignment="Right" Width="145" Height="34" Background="#F5A623" Foreground="#000000" FontWeight="Bold" FontSize="12" BorderThickness="0" Cursor="Hand">
                    <Button.Resources>
                        <Style TargetType="Border">
                            <Setter Property="CornerRadius" Value="10"/>
                        </Style>
                    </Button.Resources>
                </Button>
            </Grid>
        </Grid>
    </Border>
</Window>
"@

$reader = [System.Xml.XmlReader]::Create([System.IO.StringReader]::new($xaml))
$win = [System.Windows.Markup.XamlReader]::Load($reader)

$btnCloseX = $win.FindName("BtnCloseX")
$btnAck = $win.FindName("BtnAck")
$txtTimer = $win.FindName("TxtTimer")

$btnCloseX.Add_Click({ $win.Close() })
$btnAck.Add_Click({ $win.Close() })

# Play chime
if ($isCrit) {
    [System.Media.SystemSounds]::Hand.Play()
} else {
    [System.Media.SystemSounds]::Exclamation.Play()
}

# Auto-close timer
$script:left = $TimeoutSecs
$timer = New-Object System.Windows.Threading.DispatcherTimer
$timer.Interval = [TimeSpan]::FromSeconds(1)
$timer.Add_Tick({
    $script:left--
    if ($script:left -le 0) {
        $timer.Stop()
        $win.Close()
    } else {
        $txtTimer.Text = "$($script:left) с дараа хаагдана"
    }
})
$timer.Start()

$win.ShowDialog() | Out-Null
