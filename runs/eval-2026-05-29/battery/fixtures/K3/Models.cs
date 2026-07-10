using System.Collections.Generic;

namespace Shop
{
    public class Order { public decimal Total; public List<string> Lines = new(); }
    public class Customer { public string Email; }
}
