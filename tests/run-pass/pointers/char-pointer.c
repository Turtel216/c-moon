// A write through a `char*` touches one byte, and is visible through the
// object it points at.
//@ exit-code: 66

int main() {
    char c = 'A';
    char *p = &c;
    *p = *p + 1;
    return c;
}
