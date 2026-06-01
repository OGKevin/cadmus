# Keyboard

The on-screen keyboard lets you type text for searches, dictionary lookups, and
other input fields.

## Basics

- **Lock modifier keys** — Tap _ALT_ or _SHIFT_ twice to lock them on.
- **Combine key (CMB)** — Press _CMB_ followed by two keys to type accented or
  special characters (see [Combination Sequences](#combination-sequences)
  below). For example, `CMB o e` produces _œ_.
- **Delete / motion keys** — Tap and hold to act on whole words instead of
  individual characters.
- **Keyboard layouts** — Tap and hold the space bar to switch layouts.

## Inputting Accented Characters

Cadmus supports these through **combination sequences** using the
_CMB_ key. See [Combination Sequences](#combination-sequences).

### How to type an accented character

1. Press the **CMB** key.
2. Press the **base letter** (e.g. `e`).
3. Press the **accent modifier** (e.g. `'` for acute).

Example: `CMB e '` produces **é**.

## Combination Sequences

Press _CMB_ followed by the two keys listed below to produce the corresponding
character.

| Keys   | Result | Keys   | Result | Keys    | Result | Keys    | Result |
| ------ | ------ | ------ | ------ | ------- | ------ | ------- | ------ |
| `a '`  | á      | `A '`  | Á      | `a `` ` | à      | `A `` ` | À      |
| `e '`  | é      | `E '`  | É      | `e `` ` | è      | `E `` ` | È      |
| `i '`  | í      | `I '`  | Í      | `i `` ` | ì      | `I `` ` | Ì      |
| `o '`  | ó      | `O '`  | Ó      | `o `` ` | ò      | `O `` ` | Ò      |
| `u '`  | ú      | `U '`  | Ú      | `u `` ` | ù      | `U `` ` | Ù      |
| `y '`  | ý      | `Y '`  | Ý      | `u "`   | ű      | `U "`   | Ű      |
| `a ^`  | â      | `A ^`  | Â      | `o "`   | ő      | `O "`   | Ő      |
| `e ^`  | ê      | `E ^`  | Ê      | `a :`   | ä      | `A :`   | Ä      |
| `i ^`  | î      | `I ^`  | Î      | `e :`   | ë      | `E :`   | Ë      |
| `o ^`  | ô      | `O ^`  | Ô      | `i :`   | ï      | `I :`   | Ï      |
| `u ^`  | û      | `U ^`  | Û      | `o :`   | ö      | `O :`   | Ö      |
| `w ^`  | ŵ      | `W ^`  | Ŵ      | `u :`   | ü      | `U :`   | Ü      |
| `y ^`  | ŷ      | `Y ^`  | Ŷ      | `y :`   | ÿ      |         |        |
| `a ~`  | ã      | `A ~`  | Ã      | `c ,`   | ç      | `C ,`   | Ç      |
| `o ~`  | õ      | `O ~`  | Õ      | `c '`   | ć      | `C '`   | Ć      |
| `n ~`  | ñ      | `N ~`  | Ñ      | `z '`   | ź      | `Z '`   | Ź      |
| `a ;`  | ą      | `A ;`  | Ą      | `s '`   | ś      | `S '`   | Ś      |
| `e ;`  | ę      | `E ;`  | Ę      | `n '`   | ń      | `N '`   | Ń      |
| `z .`  | ż      | `Z .`  | Ż      | `t h`   | þ      | `T h`   | Þ      |
| `a o`  | å      | `A o`  | Å      | `l /`   | ł      | `L /`   | Ł      |
| `d /`  | đ      | `D /`  | Đ      | `o /`   | ø      | `O /`   | Ø      |
| `o e`  | œ      | `O e`  | Œ      | `a e`   | æ      | `A E`   | Æ      |
| `s s`  | ß      | `S s`  | ẞ      | `m u`   | µ      | `l -`   | £      |
| `p p`  | ¶      | `s o`  | §      | `o _`   | º      | `a _`   | ª      |
| `o o`  | °      | `e =`  | €      | `o r`   | ®     | `o c`   | ©     |
| `o p`  | ℗      | `t m`  | ™     | `] ]`   | ⟧      | `[ [`   | ⟦      |
| `\| -` | †      | `\| =` | ‡      | `- ,`   | ¬      | `~ ~`   | ≈      |
| `< <`  | «      | `> >`  | »      | `! !`   | ¡      | `? ?`   | ¿      |
| `. -`  | ·      | `. =`  | •      | `. >`   | ›      | `. <`   | ‹      |
| `' 1`  | ′      | `' 2`  | ″      | `+ -`   | ±      | `- :`   | ÷      |
| `< =`  | ≤      | `> =`  | ≥      | `= /`   | ≠      | `% o`   | ‰      |
| `# f`  | ♭      | `# n`  | ♮      | `# s`   | ♯      |         |        |
| `1 2`  | ½      | `1 3`  | ⅓      | `2 3`   | ⅔      | `1 4`   | ¼      |
| `3 4`  | ¾      | `1 5`  | ⅕      | `2 5`   | ⅖      | `3 5`   | ⅗      |
| `4 5`  | ⅘      | `1 6`  | ⅙      | `5 6`   | ⅚      | `1 8`   | ⅛      |
| `3 8`  | ⅜      | `5 8`  | ⅝      | `7 8`   | ⅞      |         |        |

## Custom Keyboard Layouts

Keyboard layouts are defined as JSON files with the following structure:

- **name** — Display name shown in the keyboard layouts menu.
- **outputs** — List of output keys for each modifier combination (_none_,
  _shift_, _alt_, _shift+alt_).
- **keys** — Description of each key. Special key names:
  _Shift_ (_Sft_), _Return_ (_Ret_), _Alternate_ (_Alt_), _Combine_ (_Cmb_),
  _MoveFwd_ (_MoveF_, _MF_), _MoveBwd_ (_MoveB_, _MB_), _DelFwd_ (_DelF_, _DF_),
  _DelBwd_ (_DelB_, _DB_), _Space_ (_Spc_). Use _▢_ to mark output keys.
- **widths** — Width/height ratio for each key. The key gap ratio is 0.06.
