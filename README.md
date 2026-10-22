# rustphp — PHP/FI 1.99s, reimplemented in Rust

The original PHP 1.x (Rasmus Lerdorf's *Personal Home Page / Forms Interpreter*,
1995–1996) was written in C. This project is a **clean-room reimplementation of
the PHP/FI 1.99s interpreter in Rust** — same surface language and observable
behaviour, but an idiomatic AST-walking interpreter instead of the original
yacc-driven streaming stack machine.

The reference C source (`museum.php.net/php2/php-1.99s.tar.gz`) was used only to
pin down exact semantics; no C was translated line-for-line.

## Build

```bash
cargo build --release
# binary at target/release/rustphp
```

Zero dependencies — only the Rust standard library.

## Tests

```bash
cargo test
```

42 tests cover the lexer (tag handling, the `<`/`>` context trick, operators,
strings/numbers/comments), the parser (echo format vs plain, if/elseif/else,
switch, assignment forms, function definitions, implicit close at EOF,
precedence), the builtins (`sprintf`, `substr`, conversions, UTC calendar math),
and the interpreter end-to-end (truthiness quirks, int-vs-float division, string
interpolation, arrays and cursors, scope/`global`/`static`, switch fall-through,
`strtok` state, error suppression, and `exit`).

## Run

**Command line** (renders a template to stdout):

```bash
rustphp examples/hello.html
```

**As CGI** (when `GATEWAY_INTERFACE` or `REQUEST_METHOD` is set, e.g. behind
Apache/nginx/busybox-httpd): GET (`QUERY_STRING`) and POST (`application/x-www-form-urlencoded`
body) fields and the whole CGI environment are exposed as global `$variables`,
and a default `Content-type: text/html` header is emitted unless the script
sends its own via `header(...)`.

**Docker:**

```bash
docker build -t rustphp .
docker run --rm rustphp                      # runs the phpinfo() demo
docker run --rm -v "$PWD/examples:/examples" rustphp /examples/hello.html
```

## The language

PHP/FI code lives between `<?` and `>` tags inside an HTML template; everything
outside a tag is emitted verbatim. Statements are separated by `;` (the closing
`>` also ends a statement). There are three scalar types — long integer, double,
string — auto-detected and coerced on demand, plus Perl-style associative
arrays.

```php
<?
$name = "world";
echo "Hello, $name!\n";          /* double-quoted strings interpolate $vars */
echo "%6.2f\n", 22.0 / 7.0;      /* printf-style format string */

if($x == 5 && $y != 0);
	echo "both\n";
elseif($x > 10);
	echo "big\n";
else;
	echo "neither\n";
endif;

$i = 0;
while($i < 3);
	echo $list[$i];
	$i++;
endwhile;

switch($v);
	case 1;    echo "one";    break;
	default;   echo "other";  break;
endswitch;

Function Sum $a,$b (          /* user-defined function: params, then (body) */
	return($a + $b);
);
echo Sum(2, 3);
>
```

Faithful quirks reproduced from the C source:

* The closing `>` is only read as *greater-than* while inside an `if`/`while`
  condition's parentheses; everywhere else it ends the tag.
* Truthiness: a number is true when non-zero, a **string is true when
  non-empty** — so `"0"` is true (unlike modern PHP).
* `10 / 3` is integer division (`3`); write `10.0 / 3` for floating point.
* Logical `&&` / `||` do **not** short-circuit (both sides are evaluated).
* Comparisons use the **left** operand's type; two strings compare with
  `strcmp`.
* Function and builtin names are case-insensitive; `$variable` names are not.

### Implemented builtins

`echo` (with printf formatting) · strings: `strlen strtoupper strtolower ucfirst
substr strstr strchr strrchr strtok sprintf htmlspecialchars urlencode
quotemeta` · math: `abs sin cos tan exp log log10 sqrt max min rand srand
getrandmax` · types: `intval doubleval strval gettype settype isset unset
bindec decbin hexdec dechex octdec decoct ord chr` · arrays: `count sort reset
next prev key end` · date/time: `time date gmdate mktime microtime` · env:
`getenv putenv` · info: `phpinfo phpversion` · control: `if/elseif/else/endif
while/endwhile switch/case/default/break/endswitch include exit header return
global static` and user-defined functions.

### Not implemented

The database, image, process and filesystem modules of the original build are
intentionally left out: `msql_* pg_*` (mSQL/Postgres95), `image*` (GD),
`dbm*`, `fopen/fgets/fputs/file/feof`, `exec/system/popen`, `crypt`,
`reg_match/reg_search/reg_replace`, directory and network functions. Calling one
prints a clear `not supported by rustphp` warning and continues.

## Project layout

| File | Role |
|------|------|
| `src/value.rs`   | the three scalar types and their coercions |
| `src/token.rs`   | token definitions |
| `src/lexer.rs`   | tag handling + tokenizer (the `<`/`>` context trick) |
| `src/ast.rs`     | AST node types |
| `src/parser.rs`  | recursive-descent parser |
| `src/interp.rs`  | tree-walking evaluator, scopes, arrays, control flow |
| `src/builtins.rs`| pure builtins, printf, calendar math |
| `src/main.rs`    | CLI / CGI front-end |

The `docker-php-1.99s/` directory is the original devilbox Docker image for the
real C build and is unrelated to the Rust port.

## License

GPL-2.0-or-later, matching the original PHP/FI.
