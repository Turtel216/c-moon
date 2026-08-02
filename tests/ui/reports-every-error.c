// Two independent undeclared names, both of which must be reported: analysis
// of a block carries on after a bad statement.

int main() {
    int first = missing_one;
    int second = missing_two;
    return first + second;
}
