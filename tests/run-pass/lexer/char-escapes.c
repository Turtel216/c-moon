// Every escape sequence the lexer understands, checked against the character
// it stands for. A failing case returns its own number, so the exit code says
// which escape is wrong.
//@ exit-code: 42

int main() {
    if ('\n' != 10) { return 1; }
    if ('\t' != 9) { return 2; }
    if ('\r' != 13) { return 3; }
    if ('\\' != 92) { return 4; }
    if ('\'' != 39) { return 5; }
    if ('\"' != 34) { return 6; }
    if ('\a' != 7) { return 7; }
    if ('\b' != 8) { return 8; }
    if ('\f' != 12) { return 9; }
    if ('\v' != 11) { return 10; }
    // `\0` is the octal escape with a single digit.
    if ('\0' != 0) { return 11; }
    if ('\7' != 7) { return 12; }
    if ('\101' != 65) { return 13; }
    // A hexadecimal escape runs for as many digits as follow it.
    if ('\x41' != 65) { return 14; }
    if ('\x0041' != 65) { return 15; }
    // A character constant is signed, so the top bit of the byte is its sign.
    if ('\xff' != 0 - 1) { return 16; }
    if ('\377' != 0 - 1) { return 17; }
    return 42;
}
