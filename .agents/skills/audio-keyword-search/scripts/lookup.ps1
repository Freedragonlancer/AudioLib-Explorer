# .agents/skills/audio-keyword-search/scripts/lookup.ps1
# Read bounded reference candidates; this is lexical retrieval, not a model call.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateLength(1, 500)][string]$Query,
    [ValidateSet('Recipe', 'Keyword', 'Ucs')][string]$Kind = 'Recipe',
    [ValidateRange(1, 30)][int]$Limit = 8
)
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($Query) -or $Query.Contains([char]0)) {
    throw 'Query must contain non-whitespace text and no NUL.'
}
$fileName = @{ Recipe = 'recipes'; Keyword = 'keywords'; Ucs = 'ucs' }[$Kind]
$dataPath = Join-Path (Split-Path $PSScriptRoot -Parent) "data/$fileName.jsonl"
$needle = $Query.ToLowerInvariant().Normalize()
$queryWords = @([regex]::Matches($needle, '[\p{L}\p{N}]+') | ForEach-Object Value)

function Get-MatchWeight([string]$Candidate, [int]$Weight) {
    if ([string]::IsNullOrWhiteSpace($Candidate)) { return 0 }
    $value = $Candidate.Trim().ToLowerInvariant().Normalize()
    if ($value.Length -lt 2 -and $value -notmatch '[\u3400-\u9fff]') { return 0 }
    if ($value -eq $needle) { return $Weight * 5 }
    # Chinese intent phrases can occur inside an unsegmented natural sentence.
    if ($needle.Contains($value)) { return $Weight * 2 }
    foreach ($word in $queryWords) {
        if (($word.Length -ge 2 -or $word -match '[\u3400-\u9fff]') -and $value.Contains($word)) { return $Weight }
    }
    return 0
}

$ranked = foreach ($line in [IO.File]::ReadLines($dataPath, [Text.Encoding]::UTF8)) {
    $row = $line | ConvertFrom-Json
    $score = 0
    switch ($Kind) {
        'Recipe' {
            $score += Get-MatchWeight $row.title 5
            foreach ($trigger in $row.triggers) { $score += Get-MatchWeight $trigger 6 }
        }
        'Keyword' {
            $score += Get-MatchWeight $row.term 8
            foreach ($term in $row.alternatives) { $score += Get-MatchWeight $term 4 }
            # Split description phrases; retaining two-character phrases supports Chinese.
            foreach ($part in ($row.zh -split '[，,、；;。/\s]+')) { $score += Get-MatchWeight $part 2 }
            $score += Get-MatchWeight $row.domain 1
        }
        'Ucs' {
            $score += Get-MatchWeight $row.id 12
            foreach ($part in @($row.category, $row.subcategory, $row.category_zh, $row.subcategory_zh)) {
                $score += Get-MatchWeight $part 5
            }
            foreach ($part in (($row.synonyms + ',' + $row.synonyms_zh) -split '[,，、;；]+')) {
                $score += Get-MatchWeight $part 1
            }
        }
    }
    if ($score -gt 0) { [pscustomobject]@{ score = $score; id = $row.id; record = $row } }
}
$items = @($ranked | Sort-Object @{Expression='score';Descending=$true},id | Select-Object -First $Limit)
[pscustomobject]@{
    kind = $Kind
    query = $Query
    scoring = 'lexical retrieval score; not probability or audio relevance'
    candidates = $items
    next_step = $(if ($items.Count) { 'Select and adapt candidates using SKILL.md; no library query was executed.' } else { 'Try the core event/material in Chinese or English; do not invent a UCS ID.' })
} | ConvertTo-Json -Depth 12
