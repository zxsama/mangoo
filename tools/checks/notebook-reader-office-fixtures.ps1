param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$out = [IO.Directory]::CreateDirectory($OutputDirectory).FullName
foreach ($name in @('writer.docx','writer.doc','ledger.xlsx','ledger.xls','slides.pptx','slides.ppt')) {
  if (Test-Path -LiteralPath ([IO.Path]::Combine($out, $name))) {
    throw "Choose a fresh fixture directory. A file named $name already exists."
  }
}

# Generate all six samples with their native Office writers. Close only the private
# automation instances created here. No user-open document is accessed.
$word = New-Object -ComObject Word.Application
try {
  $word.Visible = $false
  $word.DisplayAlerts = 0
  $doc = $word.Documents.Add()
  $doc.Content.Text = 'Notebook Word fixture'
  $doc.SaveAs2([IO.Path]::Combine($out, 'writer.docx'), 16)
  $doc.SaveAs2([IO.Path]::Combine($out, 'writer.doc'), 0)
  $doc.Close()
} finally { $word.Quit() }

$excel = New-Object -ComObject Excel.Application
try {
  $excel.Visible = $false
  $excel.DisplayAlerts = $false
  $book = $excel.Workbooks.Add(-4167)
  $a = $book.Worksheets.Item(1)
  $a.Name = 'North'
  $a.Range('A1').Value2 = 'Northern stock'
  $b = $book.Worksheets.Add([Type]::Missing, $a)
  $b.Name = 'South'
  $b.Range('A1').Value2 = 'Southern stock'
  $book.SaveAs([IO.Path]::Combine($out, 'ledger.xlsx'), 51)
  $book.SaveAs([IO.Path]::Combine($out, 'ledger.xls'), 56)
  $book.Close($false)
} finally { $excel.Quit() }

$power = New-Object -ComObject PowerPoint.Application
try {
  $power.DisplayAlerts = 1
  $deck = $power.Presentations.Add(0)
  foreach ($label in @('First plate','Second plate')) {
    $slide = $deck.Slides.Add($deck.Slides.Count + 1, 12)
    $shape = $slide.Shapes.AddTextbox(1, 55, 55, 360, 90)
    $shape.TextFrame.TextRange.Text = $label
  }
  $deck.SaveAs([IO.Path]::Combine($out, 'slides.pptx'), 24)
  $deck.SaveAs([IO.Path]::Combine($out, 'slides.ppt'), 1)
  $deck.Close()
} finally { $power.Quit() }
