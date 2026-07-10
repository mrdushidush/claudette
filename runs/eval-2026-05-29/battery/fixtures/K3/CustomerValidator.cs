namespace Shop
{
    // Decoy: validates customers, NOT orders.
    public class CustomerValidator
    {
        public bool ValidateCustomer(Customer c)
        {
            return c != null && !string.IsNullOrEmpty(c.Email);
        }
    }
}
