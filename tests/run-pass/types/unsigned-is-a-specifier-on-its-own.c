// `unsigned` and `signed` name a type by themselves -- the `int` of that
// signedness -- and may be written in front of one instead.
//@ exit-code: 7

int main() {
    unsigned u = 4294967295;
    signed s = 0 - 1;
    unsigned int spelled_out = u;
    signed int also_spelled_out = s;
    unsigned long w = u;

    return (spelled_out == u) + (also_spelled_out == s) * 2 + (w == 4294967295) * 4;
}
