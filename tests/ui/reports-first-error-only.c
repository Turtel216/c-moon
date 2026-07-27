// Two independent undeclared names, of which the semantic analyzer currently
// reports only the first.
//
// The snapshot records today's behaviour, not the desired one: when semantic
// error recovery is added this test will diff, which is the signal to re-bless
// it and confirm both names are now reported.

int main() {
    int a = b;
    int c = d;
    return a + c;
}
