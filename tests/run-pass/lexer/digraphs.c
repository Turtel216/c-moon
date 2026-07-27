// Digraphs `<:`, `:>`, `<%` and `%>` stand in for brackets and braces.
//@ exit-code: 2

int main() {
    int arr<:3:>;
    int i = 0;
    while (i < 3) <%
        arr<:i:> = i;
        i = i + 1;
    %>

    return arr<:2:>;
}
